#[derive(Debug, Clone, thiserror::Error)]
pub enum RenderError {
    #[error("Render timeout")]
    Timeout,
    #[error("Render cancelled")]
    Cancelled,
    #[error("Render panicked: {0}")]
    Panicked(String),
    #[error("HTML render error: {0}")]
    HtmlRender(String),
    #[error("Encode failed: {0}")]
    Encode(String),
    #[error("Cache error: {0}")]
    Cache(String),
    #[error("Render failed: {0}")]
    Render(String),
}
