use super::{AppId, DbusBackend, SharedState, lock_shared};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const MAX_IDENTITY_OWNERS: usize = 64;

/// How the portal obtains the application identity for one connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalIdentity {
    /// Register an unsandboxed application's installed desktop id with the host Registry.
    ///
    /// Registration precedes all portal methods and repeats for a new service
    /// owner. A portal without the Registry makes this connection unavailable;
    /// use the unbound constructor for callback-only compatibility. The
    /// connection retains at most 64 owner registrations or failed attempts;
    /// recreate the backend if this history is exhausted.
    Host,
    /// Use the identity already assigned by the portal's sandbox metadata.
    ///
    /// The configured id must match the sandbox's installed application id.
    /// Generic container environment variables do not establish this identity.
    Sandbox,
}

pub(super) struct ConnectionConfig {
    pub(super) address: Option<String>,
    pub(super) application: Option<(AppId, PortalIdentity)>,
}

impl DbusBackend {
    pub(super) async fn admit_application_owner(
        connection: &zbus::Connection,
        shared: &Arc<Mutex<SharedState>>,
        owner: String,
        application: Option<&(AppId, PortalIdentity)>,
    ) {
        let Some((application_id, PortalIdentity::Host)) = application else {
            Self::publish_owner(shared, owner);
            return;
        };
        let previous = lock_shared(shared).identity_owners.get(&owner).copied();
        if let Some(registered) = previous {
            if registered {
                Self::publish_owner(shared, owner);
            } else {
                Self::identity_failure(
                    shared,
                    "registration previously failed for this portal owner; recreate the backend"
                        .into(),
                );
            }
            return;
        }
        {
            let mut state = lock_shared(shared);
            if state.identity_owners.len() >= MAX_IDENTITY_OWNERS {
                drop(state);
                Self::identity_failure(
                    shared,
                    "portal identity owner history is full; recreate the backend".into(),
                );
                return;
            }
            state.identity_owners.insert(owner.clone(), false);
        }
        let options = HashMap::<&str, zbus::zvariant::Value<'_>>::new();
        let registration = tokio::time::timeout(
            super::ACTIVATION_ATTEMPT_TIMEOUT,
            connection.call_method(
                Some(owner.as_str()),
                super::PORTAL_PATH,
                Some("org.freedesktop.host.portal.Registry"),
                "Register",
                &(application_id.as_str(), options),
            ),
        )
        .await;
        match registration {
            Ok(Ok(_)) => {
                lock_shared(shared)
                    .identity_owners
                    .insert(owner.clone(), true);
                Self::publish_owner(shared, owner);
            }
            failure => Self::identity_failure(
                shared,
                format!("portal application registration failed: {failure:?}"),
            ),
        }
    }

    fn identity_failure(shared: &Arc<Mutex<SharedState>>, reason: String) {
        let mut state = lock_shared(shared);
        state.portal_present = Some(false);
        state.unavailable_reason = Some(reason);
        let generation = state.portal_generation;
        state
            .notification_events
            .retain(|event| !matches!(event, super::NotificationBackendEvent::OwnerChanged { .. }));
        state
            .notification_events
            .push_back(super::NotificationBackendEvent::OwnerChanged {
                generation,
                available: false,
            });
        let completion = state.completion.clone();
        drop(state);
        completion.notify();
    }
}
