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
        if error
            .code()
            .is_some_and(|code| matches!(code.code(), "23505" | "W0001"))
        {
            Self::Conflict
        } else {
            Self::Other(error.into())
        }
    }
}
