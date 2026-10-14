include!("../../shared/driver.rs");
fn load(a: &Args) -> Result<Models> {
    if a.provider == "cuda" {
        preflight()?;
    }
    let exec =
        parakeet_rs::ExecutionConfig::new().with_execution_provider(if a.provider == "cuda" {
            parakeet_rs::ExecutionProvider::Cuda
        } else {
            parakeet_rs::ExecutionProvider::Cpu
        });
    let mut asr = if a.mode != "diar" {
        Some(parakeet_rs::Nemotron::from_pretrained(
            &a.model,
            Some(exec.clone()),
        )?)
    } else {
        None
    };
    if let Some(m) = asr.as_mut() {
        if m.mode() == parakeet_rs::NemotronMode::Multilingual {
            m.set_target_lang("auto")?;
        }
    }
    let mut diar = if a.mode != "asr" {
        Some(parakeet_rs::sortformer::Sortformer::with_config(
            &a.diar,
            Some(exec),
            parakeet_rs::sortformer::DiarizationConfig::callhome(),
        )?)
    } else {
        None
    };
    if let Some(m) = diar.as_mut() {
        m.chunk_len = 6;
        m.right_context = 7;
        m.fifo_len = 188;
        m.spkcache_len = 188;
        m.reset_state();
    }
    Ok(Models { asr, diar })
}
fn preflight() -> Result<()> {
    let builder = ort::session::Session::builder().map_err(|e| anyhow::anyhow!("{e}"))?;
    let builder = builder
        .with_execution_providers([ort::execution_providers::CUDAExecutionProvider::default()
            .build()
            .error_on_failure()])
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut builder = builder
        .with_config_entry("session.disable_cpu_ep_fallback", "1")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut session = builder
        .commit_from_memory(&tiny_graph())
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let input = ort::value::Tensor::from_array(([1usize], vec![2f32]))?;
    let outputs = session.run(ort::inputs![input])?;
    let (_, values) = outputs[0].try_extract_tensor::<f32>()?;
    anyhow::ensure!(values == [4.], "CUDA preflight output mismatch");
    Ok(())
}
include!("../../shared/tiny_graph.rs");
