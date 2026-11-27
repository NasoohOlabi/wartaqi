#[derive(Debug)]
pub(crate) enum Error {
    Invalid,
    Forbidden,
    Conflict,
    NotFound,
    TooLarge,
    Other(Box<dyn std::error::Error + Send + Sync>),
}

impl From<tokio_postgres::Error> for Error {
    fn from(error: tokio_postgres::Error) -> Self {
        if error.code() == Some(&tokio_postgres::error::SqlState::UNIQUE_VIOLATION) {
            Self::Conflict
        } else {
            Self::Other(error.into())
        }
    }
}
