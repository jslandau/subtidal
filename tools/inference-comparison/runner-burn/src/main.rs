include!("../../shared/driver.rs");
fn load(a: &Args) -> Result<Models> {
    let exec =
        parakeet_rs::ExecutionConfig::new().with_execution_provider(if a.provider == "cuda" {
            parakeet_rs::ExecutionProvider::BurnCuda
        } else {
            parakeet_rs::ExecutionProvider::BurnCpu
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
            parakeet_rs::sortformer::DiarizationConfig::default(),
        )?)
    } else {
        None
    };
    if let Some(m) = diar.as_mut() {
        m.set_profile(parakeet_rs::sortformer::StreamingProfile::low_latency())?;
    }
    Ok(Models { asr, diar })
}
