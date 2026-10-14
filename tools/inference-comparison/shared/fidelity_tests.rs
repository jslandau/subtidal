#[cfg(test)]
mod fidelity_tests {
    #[test]
    fn production_baseline_contract() {
        let asr = include_str!("../../../src/stt/nemotron.rs");
        let diar = include_str!("../../../src/stt/diarization.rs");
        assert!(asr.contains("NEMOTRON_CHUNK_SAMPLES: usize = 8960"));
        assert!(diar.contains("DOCUMENTED_PRESET: (usize, usize) = (6, 7)"));
        assert!(diar.contains("RT_FIFO_LEN: usize = 188"));
        assert!(diar.contains("RT_SPKCACHE_LEN: usize = 188"));
        assert!(diar.contains("DiarizationConfig::callhome()"));
        assert!(diar.contains("inner.reset_state()"));
    }
    #[test]
    fn synthetic_stream_timing_excludes_padding_duration() {
        let mut models = super::Models {
            asr: None,
            diar: None,
        };
        let pcm = vec![0.; super::ASR + 17];
        // Warmup report is discarded; every invocation starts a new clock and counters.
        let _warmup = super::run(&mut models, &pcm).unwrap();
        let report = super::run(&mut models, &pcm).unwrap();
        assert_eq!(report["real_samples"], super::ASR + 17);
        assert_eq!(report["ticks"][0]["feed_index"], 0);
        assert_eq!(report["ticks"][0]["diar_inference_passes"], 0);
        let duration = report["duration_seconds"].as_f64().unwrap();
        assert_eq!(duration, pcm.len() as f64 / 16000.);
        assert_eq!(
            report["rtf"].as_f64().unwrap(),
            report["compute_seconds"].as_f64().unwrap() / duration
        );
        assert!(report.get("load_seconds").is_none());
        assert!(report.get("warmup_seconds").is_none());
        assert_eq!(
            report["ticks"].as_array().unwrap().len(),
            pcm.len().div_ceil(super::FEED)
        );
    }
}
