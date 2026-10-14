//! One ONNX Runtime environment shared by all production inference sessions.

use std::sync::OnceLock;

use anyhow::Result;
use ort::environment::Environment;

static ENVIRONMENT: OnceLock<Result<Environment, String>> = OnceLock::new();

/// Build GPU-first when requested, explicitly retrying CPU on construction failure.
/// The boolean reports successful GPU construction, not the caller's preference.
pub(super) fn build_with_cpu_fallback<T>(
    model: &str,
    request_gpu: bool,
    try_gpu: impl FnOnce() -> Result<T>,
    try_cpu: impl FnOnce() -> Result<T>,
) -> Result<(T, bool)> {
    if request_gpu {
        match try_gpu() {
            Ok(value) => {
                eprintln!(
                    "info: {model} using execution provider: {}",
                    gpu_provider_name()
                );
                return Ok((value, true));
            }
            Err(gpu_error) => {
                eprintln!(
                    "warn: {model} {} init failed ({gpu_error:#}); falling back to CPU",
                    gpu_provider_name()
                );
                let value = try_cpu().map_err(|cpu_error| anyhow::anyhow!(
                    "{model} GPU initialization failed: {gpu_error:#}; CPU fallback failed: {cpu_error:#}"
                ))?;
                eprintln!("info: {model} using execution provider: Cpu");
                return Ok((value, false));
            }
        }
    }
    let value = try_cpu()?;
    eprintln!("info: {model} using execution provider: Cpu");
    Ok((value, false))
}

fn gpu_provider_name() -> &'static str {
    if cfg!(target_os = "linux") {
        "Cuda"
    } else {
        "WebGPU"
    }
}

pub(super) fn execution_config(gpu: bool) -> Result<parakeet_rs::ExecutionConfig> {
    if gpu {
        require_gpu_registration()?;
    }
    #[cfg(target_os = "linux")]
    let gpu_provider = parakeet_rs::ExecutionProvider::Cuda;
    #[cfg(target_os = "macos")]
    let gpu_provider = parakeet_rs::ExecutionProvider::WebGPU;
    Ok(parakeet_rs::ExecutionConfig::new()
        .with_environment(environment()?)
        .with_execution_provider(if gpu {
            gpu_provider
        } else {
            parakeet_rs::ExecutionProvider::Cpu
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gpu_failure_attempts_cpu_and_reports_effective_provider() {
        let cpu_attempted = std::cell::Cell::new(false);
        let (value, gpu) = build_with_cpu_fallback(
            "test",
            true,
            || Err(anyhow::anyhow!("injected GPU failure")),
            || {
                cpu_attempted.set(true);
                Ok(42)
            },
        )
        .unwrap();
        assert!(cpu_attempted.get());
        assert_eq!(value, 42);
        assert!(!gpu);
    }
    #[test]
    fn both_failures_propagate() {
        let error = build_with_cpu_fallback::<()>(
            "test",
            true,
            || Err(anyhow::anyhow!("GPU failed")),
            || Err(anyhow::anyhow!("CPU failed")),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("GPU failed"));
        assert!(error.contains("CPU failed"));
    }
    #[test]
    fn gpu_success_never_attempts_cpu() {
        let (_, gpu) =
            build_with_cpu_fallback("test", true, || Ok(42), || panic!("CPU must not run"))
                .unwrap();
        assert!(gpu);
    }
    #[test]
    fn cpu_request_never_attempts_gpu() {
        let (_, gpu) =
            build_with_cpu_fallback("test", false, || panic!("GPU must not run"), || Ok(42))
                .unwrap();
        assert!(!gpu);
    }
}

/// Reject GPU provider registration failures before upstream can silently
/// fall back to CPU. Model sessions still allow CPU-only graph operators.
pub fn require_gpu_registration() -> Result<()> {
    let env = environment()?;
    let builder = ort::session::Session::builder(&env)?;
    #[cfg(target_os = "linux")]
    let provider = ort::ep::CUDA::default().build().error_on_failure();
    #[cfg(target_os = "macos")]
    let provider = ort::ep::WebGPU::default().build().error_on_failure();
    let _builder = builder
        .with_execution_providers([provider])
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    Ok(())
}

/// Return a clone of the process environment, including on engine rebuilds.
/// Initialization failure is retained so retries cannot create competing runtimes.
pub fn environment() -> Result<Environment> {
    ENVIRONMENT
        .get_or_init(|| ort::init().build().map_err(|error| error.to_string()))
        .as_ref()
        .cloned()
        .map_err(|error| anyhow::anyhow!("initializing ONNX Runtime: {error}"))
}
