use thiserror::Error;

/// Why a protocol line could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ParseError {
    #[error("line has fewer than two tokens")]
    TooShort,
    #[error("more than {} key/value pairs", crate::MAX_ARGUMENTS)]
    TooManyArguments,
    #[error("argument name without a value")]
    DanglingKey,
    #[error("token overflows a C long")]
    TokenOverflow,
    #[error("unknown command name `{0}`")]
    UnknownCommand(String),
    #[error("unknown feature name `{0}`")]
    UnknownFeature(String),
    #[error("missing argument `{0}`")]
    MissingArgument(&'static str),
    #[error("invalid value `{value}` for argument `{name}`")]
    InvalidValue { name: &'static str, value: String },
    #[error("malformed mpls list `{0}`")]
    MalformedMpls(String),
}
