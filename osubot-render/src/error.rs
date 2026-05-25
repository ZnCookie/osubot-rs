#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("Render failed: {0}")]
    Render(String),
    #[error("Encode failed: {0}")]
    Encode(String),
}
