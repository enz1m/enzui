//! The OS's observation of its background broker, for a caller check made
//! from inside a user namespace.
//!
//! There, a root-owned file shows as owned by the overflow uid, which only
//! proves it belongs to some account outside this user. The OS policy needs
//! the stronger fact, root ownership in the initial namespace, so admission
//! calls the caller descriptor's [`WitnessCall`] on the real system bus and
//! accepts its answer only when:
//!
//! - the answer came from the current owner of the witness's bus name, and
//!   the bus daemon reports that connection as uid 0;
//! - it observes this caller: the same uid, unique bus name, PID, process
//!   start time and boot, in the policy's unit;
//! - it reports the unit and executable root-owned in the initial namespace,
//!   and marks itself an observation.
//!
//! The witness is an observation, not a grant: admission asks again on every
//! call, and a missing service, an error or any mismatch refuses.

use super::admission::WitnessCall;
use serde_json::Value;

const REQUEST: &str = r#"{"schema":1}"#;

/// The caller facts admission has already authenticated on the user bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Observed<'a> {
    pub(crate) uid: u32,
    pub(crate) bus_name: &'a str,
    pub(crate) pid: u32,
    pub(crate) start_ticks: u64,
    pub(crate) boot_id: &'a str,
    pub(crate) unit: &'a str,
}

/// Why a witness was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WitnessRefusal {
    /// No system bus, no witness service, or the service refused or failed.
    Unavailable,
    /// The answer did not come from the witness name's root-owned owner.
    NotSystem,
    /// The answer is malformed or describes another process or unit.
    Mismatch,
}

/// Asks the witness service about the caller and checks the answer against
/// `observed`.
pub(crate) async fn verify(
    call: &WitnessCall,
    observed: &Observed<'_>,
) -> Result<(), WitnessRefusal> {
    let system = zbus::Connection::system()
        .await
        .map_err(|_| WitnessRefusal::Unavailable)?;
    let reply = system
        .call_method(
            Some(call.bus.as_str()),
            call.path.as_str(),
            Some(call.interface.as_str()),
            call.method.as_str(),
            &(REQUEST,),
        )
        .await
        .map_err(|_| WitnessRefusal::Unavailable)?;
    let header = reply.header();
    let sender = header.sender().ok_or(WitnessRefusal::NotSystem)?;
    let bus = zbus::fdo::DBusProxy::new(&system)
        .await
        .map_err(|_| WitnessRefusal::Unavailable)?;
    let owner = bus
        .get_name_owner(
            zbus::names::BusName::try_from(call.bus.as_str())
                .map_err(|_| WitnessRefusal::NotSystem)?,
        )
        .await
        .map_err(|_| WitnessRefusal::NotSystem)?;
    if owner.as_str() != sender.as_str() {
        return Err(WitnessRefusal::NotSystem);
    }
    let uid = bus
        .get_connection_unix_user(zbus::names::BusName::from(sender.clone()))
        .await
        .map_err(|_| WitnessRefusal::NotSystem)?;
    if uid != 0 {
        return Err(WitnessRefusal::NotSystem);
    }
    let response: String = reply
        .body()
        .deserialize()
        .map_err(|_| WitnessRefusal::Mismatch)?;
    matches(&response, observed)
}

/// Whether `response` is a schema-1 witness of exactly `observed`.
pub(crate) fn matches(response: &str, observed: &Observed<'_>) -> Result<(), WitnessRefusal> {
    let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(response) else {
        return Err(WitnessRefusal::Mismatch);
    };
    let number = |key: &str| fields.get(key).and_then(Value::as_u64);
    let text = |key: &str| fields.get(key).and_then(Value::as_str);
    let flag = |key: &str| fields.get(key).and_then(Value::as_bool);
    let agrees = number("schema") == Some(1)
        && number("uid") == Some(u64::from(observed.uid))
        && text("user_bus_name") == Some(observed.bus_name)
        && number("pid") == Some(u64::from(observed.pid))
        && number("start_ticks") == Some(observed.start_ticks)
        && text("boot_id") == Some(observed.boot_id)
        && text("unit") == Some(observed.unit)
        && flag("initial_namespace_root_owned") == Some(true)
        && flag("observation_only") == Some(true)
        && ["control_group", "unit_sha256", "executable_sha256"]
            .iter()
            .all(|key| text(key).is_some_and(|value| !value.is_empty()));
    if agrees {
        Ok(())
    } else {
        Err(WitnessRefusal::Mismatch)
    }
}

/// This boot's id, as `/proc` reports it.
pub(crate) fn boot_id() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .ok()
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observed() -> Observed<'static> {
        Observed {
            uid: 1000,
            bus_name: ":1.42",
            pid: 4242,
            start_ticks: 987_654,
            boot_id: "b0b0b0b0-0000-4000-8000-000000000001",
            unit: "example-background.service",
        }
    }

    fn witness(edit: impl FnOnce(&mut serde_json::Map<String, Value>)) -> String {
        let mut fields = serde_json::json!({
            "schema": 1,
            "uid": 1000,
            "user_bus_name": ":1.42",
            "pid": 4242,
            "start_ticks": 987_654,
            "boot_id": "b0b0b0b0-0000-4000-8000-000000000001",
            "unit": "example-background.service",
            "control_group": "/user.slice/user-1000.slice/user@1000.service/app.slice/example-background.service",
            "unit_sha256": "aa",
            "executable_sha256": "bb",
            "initial_namespace_root_owned": true,
            "observation_only": true,
        });
        edit(fields.as_object_mut().expect("object"));
        fields.to_string()
    }

    /// A witness of exactly this caller is accepted; any field describing
    /// another process, boot or unit, or a missing root observation, refuses.
    ///
    /// Sabotage: drop the `pid` comparison from `matches` and the witness of
    /// another process is accepted.
    #[test]
    fn a_witness_must_describe_exactly_the_caller() {
        assert_eq!(matches(&witness(|_| {}), &observed()), Ok(()));
        for (key, value) in [
            ("schema", serde_json::json!(2)),
            ("uid", serde_json::json!(1001)),
            ("user_bus_name", serde_json::json!(":1.43")),
            ("pid", serde_json::json!(4243)),
            ("start_ticks", serde_json::json!(987_655)),
            ("boot_id", serde_json::json!("another-boot")),
            ("unit", serde_json::json!("other.service")),
            ("initial_namespace_root_owned", serde_json::json!(false)),
            ("observation_only", serde_json::json!(false)),
            ("executable_sha256", serde_json::json!("")),
        ] {
            let response = witness(|fields| {
                fields.insert(key.to_owned(), value.clone());
            });
            assert_eq!(
                matches(&response, &observed()),
                Err(WitnessRefusal::Mismatch),
                "{key} = {value} was accepted"
            );
        }
        let missing = witness(|fields| {
            fields.remove("start_ticks");
        });
        assert_eq!(
            matches(&missing, &observed()),
            Err(WitnessRefusal::Mismatch)
        );
        assert_eq!(
            matches("not json", &observed()),
            Err(WitnessRefusal::Mismatch)
        );
    }
}
