#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("scan cancelled")]
    Cancelled,
    #[error("unsafe path: {0}")]
    Unsafe(String),
    #[error(
        "Unconfirmed cleanup plans cannot execute; prepare and explicitly confirm a new review"
    )]
    TrashDisabled,
}
