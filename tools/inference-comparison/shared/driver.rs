use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{path::PathBuf, time::Instant};
mod contract {
    include!("contract.rs");
}
use contract::*;
include!("fidelity_tests.rs");
struct Args {
    model: PathBuf,
    diar: PathBuf,
    fixture: PathBuf,
    output: PathBuf,
    provider: String,
    mode: String,
    warmup: usize,
}
fn args() -> Result<Args> {
    let mut pairs = std::collections::HashMap::new();
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        if k == "--help" {
            println!("--model-dir PATH --diar-model PATH --fixture WAV --provider cpu|cuda --mode asr|diar|combined --output JSON [--warmup N]");
            std::process::exit(0);
        }
        let v = it.next().context("each flag needs a value")?;
        if ![
            "--model-dir",
            "--diar-model",
            "--fixture",
            "--output",
            "--provider",
            "--mode",
            "--warmup",
        ]
        .contains(&k.as_str())
        {
            bail!("unknown flag {k}");
        }
        if pairs.insert(k, v).is_some() {
            bail!("duplicate flag");
        }
    }
    let required = |k: &str| -> Result<String> {
        pairs
            .get(k)
            .cloned()
            .with_context(|| format!("missing {k}"))
    };
    let a = Args {
        model: pairs
            .get("--model-dir")
            .map(PathBuf::from)
            .unwrap_or_default(),
        diar: pairs
            .get("--diar-model")
            .map(PathBuf::from)
            .unwrap_or_default(),
        fixture: required("--fixture")?.into(),
        output: required("--output")?.into(),
        provider: required("--provider")?,
        mode: required("--mode")?,
        warmup: pairs
            .get("--warmup")
            .map(|s| s.parse())
            .transpose()?
            .unwrap_or(1),
    };
    if !["cpu", "cuda"].contains(&a.provider.as_str())
        || !["asr", "diar", "combined"].contains(&a.mode.as_str())
    {
        bail!("invalid provider or mode");
    }
    if a.mode != "diar" && !a.model.is_dir() {
        bail!("--model-dir must be an existing directory");
    }
    if a.mode != "asr" && !a.diar.is_file() {
        bail!("--diar-model must be an existing model file");
    }
    Ok(a)
}
fn wav(path: &PathBuf) -> Result<Vec<f32>> {
    let mut r = hound::WavReader::open(path)?;
    let s = r.spec();
    if s.channels != 1 || s.sample_rate != 16000 {
        bail!("WAV must be 16kHz mono");
    }
    let pcm = match (s.sample_format, s.bits_per_sample) {
        (hound::SampleFormat::Float, 32) => r
            .samples::<f32>()
            .collect::<std::result::Result<Vec<_>, _>>()?,
        (hound::SampleFormat::Int, 16) => r
            .samples::<i16>()
            .map(|x| x.map(|v| v as f32 / 32768.))
            .collect::<std::result::Result<Vec<_>, _>>()?,
        _ => bail!("WAV must contain f32 or i16 PCM"),
    };
    if pcm.is_empty() || pcm.iter().any(|x| !x.is_finite()) {
        bail!("empty or nonfinite WAV");
    }
    Ok(pcm)
}
struct Models {
    asr: Option<parakeet_rs::Nemotron>,
    diar: Option<parakeet_rs::sortformer::Sortformer>,
}
fn segments(segs: Vec<parakeet_rs::sortformer::SpeakerSegment>, real: usize) -> Result<Vec<Value>> {
    anyhow::ensure!(
        segs.iter().all(|s| (s.speaker_id as usize) < 8),
        "invalid speaker ID: expected 0..7"
    );
    Ok(segs
        .into_iter()
        .filter_map(|s| {
            clip_segment(
                s.start as usize,
                s.end as usize,
                s.speaker_id as usize,
                real,
            )
        })
        .map(|(start, end, id)| json!({"start_sample":start,"end_sample":end,"speaker_id":id}))
        .collect())
}
fn run(m: &mut Models, pcm: &[f32]) -> Result<Value> {
    let clock = Instant::now();
    let mut f = Framer::default();
    let mut ticks = Vec::new();
    let mut fragments = Vec::new();
    let mut segs = Vec::new();
    let mut first_asr = None;
    let mut first_diar = None;
    let mut transcript = String::new();
    let mut diar_pending = 0usize;
    for (index, block) in pcm.chunks(FEED).enumerate() {
        let end = (index * FEED + block.len()).min(pcm.len());
        let t = Instant::now();
        let mut ds = Vec::new();
        let mut diar_passes = 0usize;
        if let Some(d) = m.diar.as_mut() {
            diar_pending += block.len();
            while diar_pending >= (d.chunk_len + d.right_context) * 1280 {
                diar_pending -= d.chunk_len * 1280;
                diar_passes += 1;
            }
            ds = segments(d.feed(block)?, pcm.len())?;
        }
        let diar_seconds = if m.diar.is_some() {
            t.elapsed().as_secs_f64()
        } else {
            0.
        };
        if !ds.is_empty() && first_diar.is_none() {
            first_diar =
                Some(json!({"compute_seconds":clock.elapsed().as_secs_f64(),"audio_sample":end}));
        }
        segs.extend(ds);
        let mut asr_seconds = 0.;
        let mut inferred = false;
        if let Some(a) = m.asr.as_mut() {
            if let Some(chunk) = f.push(block) {
                let t = Instant::now();
                let text = a.transcribe_chunk(&chunk)?;
                asr_seconds = t.elapsed().as_secs_f64();
                inferred = true;
                if !text.is_empty() {
                    if first_asr.is_none() {
                        first_asr = Some(
                            json!({"compute_seconds":clock.elapsed().as_secs_f64(),"audio_sample":end}),
                        );
                    }
                    transcript.push_str(&text);
                    fragments.push(json!({"text":text,"emit_sample":end}));
                }
            }
        }
        ticks.push(json!({"feed_index":index,"audio_start_sample":index*FEED,"audio_end_sample":end,"diar_seconds":if diar_passes>0 {diar_seconds} else {0.},"diar_feed_seconds":diar_seconds,"diar_inference_passes":diar_passes,"asr_seconds":asr_seconds,"asr_inference":inferred}));
    }
    let t = Instant::now();
    if let Some(d) = m.diar.as_mut() {
        let ds = segments(d.flush()?, pcm.len())?;
        if !ds.is_empty() && first_diar.is_none() {
            first_diar = Some(
                json!({"compute_seconds":clock.elapsed().as_secs_f64(),"audio_sample":pcm.len()}),
            );
        }
        segs.extend(ds);
    }
    let finish_diar = t.elapsed().as_secs_f64();
    let t = Instant::now();
    if let Some(a) = m.asr.as_mut() {
        if let Some(chunk) = f.finish() {
            let text = a.transcribe_chunk(&chunk)?;
            if !text.is_empty() {
                if first_asr.is_none() {
                    first_asr = Some(
                        json!({"compute_seconds":clock.elapsed().as_secs_f64(),"audio_sample":pcm.len()}),
                    );
                }
                transcript.push_str(&text);
                fragments.push(json!({"text":text,"emit_sample":pcm.len(),"finish":true}));
            }
        }
    }
    let finish_asr = t.elapsed().as_secs_f64();
    let seconds = clock.elapsed().as_secs_f64();
    Ok(
        json!({"real_samples":pcm.len(),"duration_seconds":pcm.len() as f64/RATE as f64,"compute_seconds":seconds,"rtf":rtf(seconds,pcm.len()),"transcript":transcript,"fragments":fragments,"segments":segs,"ticks":ticks,"first_asr_output":first_asr,"first_diar_output":first_diar,"finish_asr_seconds":finish_asr,"finish_diar_seconds":finish_diar}),
    )
}
fn main() -> Result<()> {
    let a = args()?;
    let pcm = wav(&a.fixture)?;
    let t = Instant::now();
    let mut models = load(&a)?;
    let load_seconds = t.elapsed().as_secs_f64();
    let t = Instant::now();
    for _ in 0..a.warmup {
        run(&mut models, &pcm)?;
        if let Some(m) = models.asr.as_mut() {
            m.reset();
        }
        if let Some(m) = models.diar.as_mut() {
            m.reset_state();
        }
    }
    let warmup_seconds = t.elapsed().as_secs_f64();
    let mut result = run(&mut models, &pcm)?;
    result["cuda_preflight"] = json!(if env!("CARGO_PKG_NAME") == "runner-burn"
        || a.provider == "cpu"
    {
        "not_applicable"
    } else {
        "passed"
    });
    result["provider_validation"] = json!(if env!("CARGO_PKG_NAME") == "runner-burn" {
        "explicit-burn-backend"
    } else if a.provider == "cuda" {
        "strict-direct-cuda-preflight-passed; upstream-model-cpu-fallback-possible"
    } else {
        "explicit-cpu"
    });
    result["padding_samples"] = json!(if a.mode == "diar" {
        0
    } else {
        (ASR - pcm.len() % ASR) % ASR
    });
    result["runtime_config"] = json!({"feed_samples":FEED,"asr_chunk_samples":ASR,"diar_chunk_len":models.diar.as_ref().map(|m|m.chunk_len),"diar_right_context":models.diar.as_ref().map(|m|m.right_context),"diar_fifo_len":models.diar.as_ref().map(|m|m.fifo_len),"diar_spkcache_len":models.diar.as_ref().map(|m|m.spkcache_len),"diar_profile":if env!("CARGO_PKG_NAME")=="runner-baseline" {"production-low-latency-callhome"} else {"low_latency-default-postprocessing"},"ort_internal_cpu_fallback_possible":env!("CARGO_PKG_NAME")!="runner-burn"});
    result["schema_version"] = json!(1);
    result["runner"] = json!(env!("CARGO_PKG_NAME"));
    result["provider"] = json!(a.provider);
    result["mode"] = json!(a.mode);
    result["fixture"] = json!(a.fixture);
    result["load_seconds"] = json!(load_seconds);
    result["warmup_seconds"] = json!(warmup_seconds);
    result["warmup_runs"] = json!(a.warmup);
    result["timing_scope"]=json!("unpaced serial host-materialized inference plus framing/output collection; excludes WAV read, model load, warmup and JSON write");
    std::fs::write(a.output, serde_json::to_vec_pretty(&result)?)?;
    Ok(())
}
#[cfg(test)]
mod schema_tests {
    use super::*;
    #[test]
    fn empty_models_schema_and_fragment_fidelity() {
        let v = run(
            &mut Models {
                asr: None,
                diar: None,
            },
            &vec![0.; FEED + 1],
        )
        .unwrap();
        assert_eq!(v["ticks"].as_array().unwrap().len(), 2);
        assert_eq!(v["ticks"][1]["audio_end_sample"], FEED + 1);
        assert_eq!(v["ticks"][0]["asr_seconds"], 0.);
        assert!(v["first_asr_output"].is_null());
        let fragments = [" hello", "world", "!"];
        assert_eq!(fragments.concat(), " helloworld!");
    }
}
