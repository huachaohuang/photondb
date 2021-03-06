use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Alloc")]
    Alloc,
    #[error("Again")]
    Again,
    #[error("Corrupted: {0}")]
    Corrupted(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
