//! Typed, explicitly registered app functions and their registration metadata.
//!
//! An app declares each function once with a validated id, an object-root JSON
//! schema, a risk level, and the consent scopes it is willing to accept. The
//! declared consent set and capabilities are an upper bound: the daemon
//! remains the authority on what any given call is actually allowed to do. A
//! function maps validated JSON arguments to the app's own `Message` type, so
//! IPC code never touches the model directly. Registration is explicit; there
//! is no global inventory or linker registration.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::center::protocol::{
    AppMetadata, AppRegistration, DEFAULT_RESULT_BYTES, FunctionDescriptor, Invocation,
    MAX_RESULT_BYTES,
};
pub use crate::center::protocol::{ConsentScope, FunctionRisk};

/// Maximum functions one app may register, matching the daemon.
pub const MAX_FUNCTIONS_PER_APP: usize = 256;

/// Maximum serialized bytes for one input or output schema, matching the daemon.
pub const MAX_SCHEMA_BYTES: usize = 128 * 1024;

/// Maximum id length in bytes, matching the daemon.
pub const MAX_ID_BYTES: usize = 128;

/// Maximum bytes for a function title or app name, matching the daemon.
pub const MAX_TITLE_BYTES: usize = 128;

/// Maximum bytes for a function or app description, matching the daemon.
pub const MAX_DESCRIPTION_BYTES: usize = 512;

/// Maximum bytes for an app version string, matching the daemon.
pub const MAX_VERSION_BYTES: usize = 64;

/// Maximum required capabilities per function, matching the daemon.
pub const MAX_REQUIRED_CAPABILITIES: usize = 64;

/// Maximum acceptable consent scopes per function, matching the daemon.
pub const MAX_CONSENT_SCOPES: usize = 3;

/// Why a function or app declaration was refused before it could reach the wire.
#[derive(Debug, PartialEq, Eq)]
pub enum RegistryError {
    /// An id was empty, too long, or contained a character outside
    /// `[A-Za-z0-9._-]`.
    InvalidId {
        /// The rejected id.
        id: String,
    },
    /// A function with this id was already registered.
    DuplicateId {
        /// The duplicated id.
        id: String,
    },
    /// More than [`MAX_FUNCTIONS_PER_APP`] functions were declared.
    TooManyFunctions {
        /// The count that exceeded the bound.
        count: usize,
    },
    /// An input or output schema did not have a `"type": "object"` root, which
    /// the daemon requires.
    SchemaNotObject {
        /// The function id whose schema was rejected.
        id: String,
    },
    /// An input or output schema serialized to more than [`MAX_SCHEMA_BYTES`].
    SchemaTooLarge {
        /// The function id whose schema was rejected.
        id: String,
        /// The serialized schema size.
        bytes: usize,
    },
    /// A declared result byte budget was zero or exceeded [`MAX_RESULT_BYTES`].
    InvalidResultBudget {
        /// The function id.
        id: String,
        /// The rejected budget.
        bytes: u32,
    },
    /// A function declared no acceptable consent scope, or more than the three
    /// the daemon accepts.
    InvalidConsent {
        /// The function id.
        id: String,
        /// The rejected scope count.
        count: usize,
    },
    /// A function declared more than 64 required capabilities, or a capability
    /// that is not a valid identifier.
    InvalidCapabilities {
        /// The function id.
        id: String,
    },
    /// A title, description, name, or version was empty, too long, or held a
    /// control character other than tab or newline.
    InvalidText {
        /// The function or app id the text belongs to.
        subject: String,
        /// Which field was rejected.
        field: &'static str,
    },
}

impl core::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidId { id } => write!(f, "invalid id {id:?}"),
            Self::DuplicateId { id } => write!(f, "duplicate function id {id:?}"),
            Self::TooManyFunctions { count } => {
                write!(
                    f,
                    "{count} functions exceeds the {MAX_FUNCTIONS_PER_APP} bound"
                )
            }
            Self::SchemaNotObject { id } => write!(f, "schema for {id:?} is not an object root"),
            Self::SchemaTooLarge { id, bytes } => {
                write!(f, "schema for {id:?} is {bytes} bytes over the bound")
            }
            Self::InvalidResultBudget { id, bytes } => {
                write!(
                    f,
                    "result budget {bytes} for {id:?} is zero or over the bound"
                )
            }
            Self::InvalidConsent { id, count } => {
                write!(f, "function {id:?} declares {count} consent scopes")
            }
            Self::InvalidCapabilities { id } => {
                write!(
                    f,
                    "function {id:?} declares invalid or too many capabilities"
                )
            }
            Self::InvalidText { subject, field } => {
                write!(
                    f,
                    "{field} for {subject:?} is empty, too long, or has control characters"
                )
            }
        }
    }
}

impl std::error::Error for RegistryError {}

/// Why a dispatched invocation could not be mapped to a typed message.
#[derive(Debug)]
pub enum DispatchError {
    /// No function with the invocation's id is registered.
    UnknownFunction {
        /// The unknown function id.
        id: String,
    },
    /// The arguments did not match the function's declared input type. The
    /// underlying serde error is not surfaced, because it can echo argument
    /// values.
    Arguments {
        /// The function id whose arguments were rejected.
        id: String,
    },
}

impl core::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnknownFunction { id } => write!(f, "unknown function {id:?}"),
            Self::Arguments { id } => write!(f, "arguments for {id:?} did not validate"),
        }
    }
}

impl std::error::Error for DispatchError {}

/// Whether `id` is 1..=[`MAX_ID_BYTES`] bytes of `[A-Za-z0-9._-]`.
#[must_use]
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_BYTES
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Whether `value` is a daemon-acceptable free-text field: non-empty after
/// trimming, at most `max` bytes, and free of control characters other than
/// tab and newline.
#[must_use]
pub fn valid_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= max
        && !value
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
}

/// A closed JSON-schema object for a function's arguments or result: these
/// `properties`, the `required` ones among them, and nothing else
/// (`"additionalProperties": false`), so an agent cannot pass fields the app
/// would silently ignore.
///
/// ```
/// use serde_json::json;
/// let schema = enzui_agent::center::object_schema(
///     json!({"query": {"type": "string"}, "limit": {"type": "integer"}}),
///     &["query"],
/// );
/// assert_eq!(schema["additionalProperties"], json!(false));
/// ```
#[must_use]
pub fn object_schema(properties: Value, required: &[&str]) -> Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": required,
        "properties": properties,
    })
}

/// A declaration of one function, validated into a [`FunctionDescriptor`].
///
/// Build with [`FunctionSpec::new`], set optional fields, then hand it to
/// [`AppFunction::new`] or [`AppFunction::typed`].
#[derive(Clone, Debug)]
pub struct FunctionSpec {
    id: String,
    title: String,
    description: String,
    input_schema: Value,
    output_schema: Value,
    risk: FunctionRisk,
    allowed_consent: Vec<ConsentScope>,
    required_capabilities: Vec<String>,
    supports_cancellation: bool,
    supports_progress: bool,
    max_result_bytes: u32,
}

impl FunctionSpec {
    /// Starts a declaration with the required fields. Schemas must be JSON
    /// objects at their root.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
        output_schema: Value,
        risk: FunctionRisk,
        allowed_consent: Vec<ConsentScope>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            description: description.into(),
            input_schema,
            output_schema,
            risk,
            allowed_consent,
            required_capabilities: Vec::new(),
            supports_cancellation: false,
            supports_progress: false,
            max_result_bytes: DEFAULT_RESULT_BYTES,
        }
    }

    /// Declares the platform capabilities this function needs. These are an
    /// upper bound the daemon and blueprint validate; they never grant a
    /// capability.
    #[must_use]
    pub fn required_capabilities(mut self, capabilities: Vec<String>) -> Self {
        self.required_capabilities = capabilities;
        self
    }

    /// Declares that the app honours cancellation for this function.
    #[must_use]
    pub fn supports_cancellation(mut self, value: bool) -> Self {
        self.supports_cancellation = value;
        self
    }

    /// Declares that the app emits progress events for this function.
    #[must_use]
    pub fn supports_progress(mut self, value: bool) -> Self {
        self.supports_progress = value;
        self
    }

    /// Sets the maximum result size the app will return for this function.
    #[must_use]
    pub fn max_result_bytes(mut self, bytes: u32) -> Self {
        self.max_result_bytes = bytes;
        self
    }

    fn build(self) -> Result<FunctionDescriptor, RegistryError> {
        if !valid_id(&self.id) {
            return Err(RegistryError::InvalidId { id: self.id });
        }
        if !valid_text(&self.title, MAX_TITLE_BYTES) {
            return Err(RegistryError::InvalidText {
                subject: self.id,
                field: "title",
            });
        }
        if !valid_text(&self.description, MAX_DESCRIPTION_BYTES) {
            return Err(RegistryError::InvalidText {
                subject: self.id,
                field: "description",
            });
        }
        if self.allowed_consent.is_empty() || self.allowed_consent.len() > MAX_CONSENT_SCOPES {
            return Err(RegistryError::InvalidConsent {
                id: self.id,
                count: self.allowed_consent.len(),
            });
        }
        if self.required_capabilities.len() > MAX_REQUIRED_CAPABILITIES
            || !self.required_capabilities.iter().all(|c| valid_id(c))
        {
            return Err(RegistryError::InvalidCapabilities { id: self.id });
        }
        if self.max_result_bytes == 0 || self.max_result_bytes > MAX_RESULT_BYTES {
            return Err(RegistryError::InvalidResultBudget {
                id: self.id,
                bytes: self.max_result_bytes,
            });
        }
        for schema in [&self.input_schema, &self.output_schema] {
            if schema.get("type").and_then(Value::as_str) != Some("object") {
                return Err(RegistryError::SchemaNotObject { id: self.id });
            }
            let bytes = serde_json::to_vec(schema).map_or(usize::MAX, |v| v.len());
            if bytes > MAX_SCHEMA_BYTES {
                return Err(RegistryError::SchemaTooLarge { id: self.id, bytes });
            }
        }
        Ok(FunctionDescriptor {
            id: self.id,
            title: self.title,
            description: self.description,
            input_schema: self.input_schema,
            output_schema: self.output_schema,
            risk: self.risk,
            allowed_consent: self.allowed_consent,
            required_capabilities: self.required_capabilities,
            supports_cancellation: self.supports_cancellation,
            supports_progress: self.supports_progress,
            max_result_bytes: self.max_result_bytes,
        })
    }
}

type Mapper<M> = Arc<dyn Fn(Value) -> Result<M, ()> + Send + Sync>;

/// A registered function: its wire descriptor plus a mapping from validated
/// arguments to the app's `Message` type.
pub struct AppFunction<M> {
    descriptor: FunctionDescriptor,
    map: Mapper<M>,
}

impl<M: 'static> AppFunction<M> {
    /// Builds a function whose mapper receives the raw JSON arguments.
    ///
    /// # Errors
    /// Returns [`RegistryError`] when the spec fails validation.
    pub fn new(
        spec: FunctionSpec,
        map: impl Fn(Value) -> M + Send + Sync + 'static,
    ) -> Result<Self, RegistryError> {
        let descriptor = spec.build()?;
        Ok(Self {
            descriptor,
            map: Arc::new(move |value| Ok(map(value))),
        })
    }

    /// Builds a function whose mapper receives strongly-typed, schema-validated
    /// input decoded from the JSON arguments.
    ///
    /// # Errors
    /// Returns [`RegistryError`] when the spec fails validation. At dispatch
    /// time, arguments that do not decode to `I` are refused with
    /// [`DispatchError::Arguments`].
    pub fn typed<I: DeserializeOwned>(
        spec: FunctionSpec,
        map: impl Fn(I) -> M + Send + Sync + 'static,
    ) -> Result<Self, RegistryError> {
        let descriptor = spec.build()?;
        Ok(Self {
            descriptor,
            map: Arc::new(move |value| {
                serde_json::from_value::<I>(value).map(&map).map_err(|_| ())
            }),
        })
    }

    /// The wire descriptor.
    #[must_use]
    pub fn descriptor(&self) -> &FunctionDescriptor {
        &self.descriptor
    }
}

/// An ordered set of registered functions for one app.
pub struct AppFunctionRegistry<M> {
    functions: Vec<AppFunction<M>>,
    ids: BTreeSet<String>,
}

impl<M: 'static> Default for AppFunctionRegistry<M> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M: 'static> AppFunctionRegistry<M> {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            functions: Vec::new(),
            ids: BTreeSet::new(),
        }
    }

    /// Registers `function`, rejecting a duplicate id or an over-full registry.
    ///
    /// # Errors
    /// Returns [`RegistryError::DuplicateId`] or
    /// [`RegistryError::TooManyFunctions`].
    pub fn register(&mut self, function: AppFunction<M>) -> Result<(), RegistryError> {
        let id = function.descriptor.id.clone();
        if self.ids.contains(&id) {
            return Err(RegistryError::DuplicateId { id });
        }
        if self.functions.len() >= MAX_FUNCTIONS_PER_APP {
            return Err(RegistryError::TooManyFunctions {
                count: self.functions.len() + 1,
            });
        }
        self.ids.insert(id);
        self.functions.push(function);
        Ok(())
    }

    /// Builds a registry from an iterator, failing on the first invalid entry.
    ///
    /// # Errors
    /// Returns the first [`RegistryError`].
    pub fn from_functions(
        functions: impl IntoIterator<Item = AppFunction<M>>,
    ) -> Result<Self, RegistryError> {
        let mut registry = Self::new();
        for function in functions {
            registry.register(function)?;
        }
        Ok(registry)
    }

    /// The number of registered functions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.functions.len()
    }

    /// Whether no functions are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }

    /// Builds the wire registration for `app`.
    ///
    /// # Errors
    /// Returns [`RegistryError::InvalidId`] when the app id is malformed or
    /// [`RegistryError::TooManyFunctions`] when too many functions are declared.
    pub fn registration(&self, app: AppMetadata) -> Result<AppRegistration, RegistryError> {
        if !valid_id(&app.id) {
            return Err(RegistryError::InvalidId { id: app.id });
        }
        if !valid_text(&app.name, MAX_TITLE_BYTES) {
            return Err(RegistryError::InvalidText {
                subject: app.id,
                field: "name",
            });
        }
        if !valid_text(&app.description, MAX_DESCRIPTION_BYTES) {
            return Err(RegistryError::InvalidText {
                subject: app.id,
                field: "description",
            });
        }
        if !valid_text(&app.version, MAX_VERSION_BYTES) {
            return Err(RegistryError::InvalidText {
                subject: app.id,
                field: "version",
            });
        }
        if self.functions.len() > MAX_FUNCTIONS_PER_APP {
            return Err(RegistryError::TooManyFunctions {
                count: self.functions.len(),
            });
        }
        Ok(AppRegistration {
            app,
            functions: self
                .functions
                .iter()
                .map(|function| function.descriptor.clone())
                .collect(),
        })
    }

    /// The descriptor registered under `id`, if any.
    #[must_use]
    pub fn descriptor(&self, id: &str) -> Option<&FunctionDescriptor> {
        self.functions
            .iter()
            .map(AppFunction::descriptor)
            .find(|descriptor| descriptor.id == id)
    }

    /// Maps a dispatched invocation to the app's typed message.
    ///
    /// # Errors
    /// Returns [`DispatchError::UnknownFunction`] for an unregistered id or
    /// [`DispatchError::Arguments`] when the arguments do not validate.
    pub fn dispatch(&self, invocation: &Invocation) -> Result<M, DispatchError> {
        let function = self
            .functions
            .iter()
            .find(|function| function.descriptor.id == invocation.function_id)
            .ok_or_else(|| DispatchError::UnknownFunction {
                id: invocation.function_id.clone(),
            })?;
        (function.map)(invocation.arguments.clone()).map_err(|()| DispatchError::Arguments {
            id: invocation.function_id.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn object_schema() -> Value {
        json!({"type": "object", "additionalProperties": false})
    }

    fn spec(id: &str) -> FunctionSpec {
        FunctionSpec::new(
            id,
            "Title",
            "Description",
            object_schema(),
            object_schema(),
            FunctionRisk::ReadOnly,
            vec![ConsentScope::Once],
        )
    }

    #[derive(Debug, PartialEq)]
    enum Message {
        Raw,
    }

    #[test]
    fn duplicate_registration_is_rejected() {
        let mut registry = AppFunctionRegistry::<Message>::new();
        registry
            .register(AppFunction::new(spec("echo"), |_| Message::Raw).expect("build"))
            .expect("first insert");
        let error = registry
            .register(AppFunction::new(spec("echo"), |_| Message::Raw).expect("build"))
            .expect_err("second must fail");
        assert_eq!(error, RegistryError::DuplicateId { id: "echo".into() });
    }
}
