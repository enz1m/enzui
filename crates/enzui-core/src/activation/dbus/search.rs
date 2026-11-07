//! An app's results in the desktop's Search: enzUI's
//! `dev.enzui.SearchProvider1`, GNOME Shell's `org.gnome.Shell.SearchProvider2`
//! and KRunner's `org.kde.krunner1`, all answered by one provider.
//!
//! The desktop asks every running provider for results on each typing pause
//! and shows a few under the app's name; choosing one asks the app to open it.
//! An app gives a [`SearchProvider`] to
//! [`ApplicationService::serve_search`](super::ApplicationService::serve_search),
//! which serves the three interfaces beside `org.freedesktop.Application` on
//! the app's own bus name and object path, and takes the user's choices from
//! the returned
//! [`SearchInbox`] when [`wake_subscription`](super::ApplicationService::wake_subscription)
//! fires.
//!
//! - Search is windowless. The provider only returns results: it must not
//!   map, raise or focus a window, or notify. Only an activation brings up UI.
//! - Search runs on every typing pause, so keep it cheap. A newer query
//!   from the same caller cancels the one still running, and a search past
//!   [`SEARCH_DEADLINE`] answers nothing, since the desktop has stopped
//!   waiting.
//! - Results are bounded by construction. [`SearchResult::new`] refuses
//!   an id or title the desktop would drop, and shortens long text on a
//!   character boundary.
//!
//! [`search_provider_ini`] writes the declaration enzUI's desktop discovers
//! the provider by, from the same app ID and path the service serves;
//! `crate::packaging::AppPackage::search_provider` installs GNOME Shell's and
//! KRunner's. GNOME asks for a result's details, and KRunner opens one, by id
//! alone: recent results are remembered for them.

use super::{
    ActivationBroker, ActivationContext, InboundActivationToken, InboundTokenError,
    application_object_path,
};
use crate::portal::CompletionSignal;
use iced_futures::futures::future::{self, Either};
use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::future::Future;
use std::pin::{Pin, pin};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use zbus::zvariant::{OwnedValue, Value};

/// The D-Bus interface the desktop's Search calls.
pub const SEARCH_INTERFACE: &str = "dev.enzui.SearchProvider1";
/// The most results one answer carries; the desktop ignores the rest.
pub const MAX_SEARCH_RESULTS: usize = 20;
/// The results asked for when the desktop names no limit.
pub const DEFAULT_SEARCH_LIMIT: usize = 5;
/// The longest result id, in bytes; a longer id is refused, not cut.
pub const MAX_RESULT_ID_BYTES: usize = 256;
/// The longest title or subtitle, in bytes; longer text is shortened.
pub const MAX_RESULT_TEXT_BYTES: usize = 512;
/// The longest themed icon name, in bytes.
pub const MAX_RESULT_ICON_BYTES: usize = 128;
/// The longest query a provider is asked; a longer one answers nothing.
pub const MAX_QUERY_BYTES: usize = 1024;
/// How long the desktop waits for an answer; work past it is dropped.
pub const SEARCH_DEADLINE: Duration = Duration::from_millis(1500);
/// Activations kept while the app is not draining.
pub const SEARCH_ACTIVATION_CAPACITY: usize = 16;

/// Why [`SearchResult::new`] refused a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchResultError {
    /// The id is empty or longer than [`MAX_RESULT_ID_BYTES`].
    Id,
    /// The title is empty, or has a control character other than a tab or a
    /// line break.
    Title,
}

impl std::fmt::Display for SearchResultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Id => "a search result id must be 1 to 256 bytes",
            Self::Title => "a search result title must be non-empty plain text",
        })
    }
}

impl std::error::Error for SearchResultError {}

/// One result as the desktop shows it: a title, an optional subtitle, icon
/// and relevance, and the id the app gets back when it is chosen.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    id: String,
    title: String,
    subtitle: Option<String>,
    icon: Option<String>,
    score: Option<f64>,
}

impl SearchResult {
    /// A result with `id`, which only the app reads, and `title`.
    ///
    /// # Errors
    ///
    /// An empty or oversized id, or a title the desktop would drop.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Result<Self, SearchResultError> {
        let id = id.into();
        if id.is_empty() || id.len() > MAX_RESULT_ID_BYTES {
            return Err(SearchResultError::Id);
        }
        let title = title.into();
        if title.trim().is_empty() || !plain(&title) {
            return Err(SearchResultError::Title);
        }
        Ok(Self {
            id,
            title: shorten(title),
            subtitle: None,
            icon: None,
            score: None,
        })
    }

    /// A second line, such as a sender or a date. Control characters other
    /// than tabs and line breaks are removed.
    #[must_use]
    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        let subtitle: String = subtitle
            .into()
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\t' | '\n' | '\r'))
            .collect();
        self.subtitle = (!subtitle.trim().is_empty()).then(|| shorten(subtitle));
        self
    }

    /// A freedesktop themed icon name; the app's own icon is shown without
    /// one, or for a name that is not one.
    #[must_use]
    pub fn icon(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        let themed = !name.is_empty()
            && name.len() <= MAX_RESULT_ICON_BYTES
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        self.icon = themed.then_some(name);
        self
    }

    /// Relevance from `0` to `1`, clamped; ignored when not a number.
    #[must_use]
    pub fn score(mut self, score: f64) -> Self {
        self.score = score.is_finite().then(|| score.clamp(0.0, 1.0));
        self
    }

    /// The id the app gets back when this result is chosen.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    fn to_wire(&self) -> HashMap<String, OwnedValue> {
        let mut map = HashMap::new();
        let mut put = |key: &str, value: Value<'_>| {
            if let Ok(value) = value.try_to_owned() {
                map.insert(key.to_owned(), value);
            }
        };
        put("id", Value::from(self.id.as_str()));
        put("title", Value::from(self.title.as_str()));
        if let Some(subtitle) = &self.subtitle {
            put("subtitle", Value::from(subtitle.as_str()));
        }
        if let Some(icon) = &self.icon {
            put("icon", Value::from(icon.as_str()));
        }
        if let Some(score) = self.score {
            put("score", Value::from(score));
        }
        map
    }
}

fn plain(text: &str) -> bool {
    text.chars()
        .all(|c| !c.is_control() || matches!(c, '\t' | '\n' | '\r'))
}

fn shorten(mut text: String) -> String {
    if text.len() > MAX_RESULT_TEXT_BYTES {
        let mut end = MAX_RESULT_TEXT_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

/// What the desktop asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    /// The text typed, as typed.
    pub text: String,
    /// How many results to return, at most [`MAX_SEARCH_RESULTS`].
    pub limit: usize,
    /// The desktop's locale, such as `de_DE`, when it said.
    pub locale: Option<String>,
}

type Run =
    dyn Fn(SearchQuery) -> Pin<Box<dyn Future<Output = Vec<SearchResult>> + Send>> + Send + Sync;

/// How an app answers a search. Cheap to clone.
#[derive(Clone)]
pub struct SearchProvider {
    run: Arc<Run>,
}

impl std::fmt::Debug for SearchProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchProvider").finish_non_exhaustive()
    }
}

impl SearchProvider {
    /// A provider whose search is a future, for apps whose data is already
    /// async. Dropping the future cancels the search.
    pub fn new<F, Fut>(search: F) -> Self
    where
        F: Fn(SearchQuery) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Vec<SearchResult>> + Send + 'static,
    {
        Self {
            run: Arc::new(move |query| Box::pin(search(query))),
        }
    }

    /// A provider whose search blocks, such as a database query. It runs on
    /// a blocking thread; a superseded or late search stops being waited for
    /// but runs to its end, so keep it bounded.
    pub fn blocking<F>(search: F) -> Self
    where
        F: Fn(&SearchQuery) -> Vec<SearchResult> + Send + Sync + 'static,
    {
        let search = Arc::new(search);
        Self::new(move |query| {
            let search = Arc::clone(&search);
            async move {
                tokio::task::spawn_blocking(move || search(&query))
                    .await
                    .unwrap_or_default()
            }
        })
    }
}

/// What the user chose in the desktop's Search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchRequest {
    /// Open the result with this id, found for `query`.
    Open {
        /// The [`SearchResult::id`].
        id: String,
        /// The text that found it.
        query: String,
    },
    /// Show the app's own search for `query`.
    Search {
        /// The text typed.
        query: String,
    },
}

/// One activation ready for the app: open the result or the search, and
/// raise the window with `context` when there is one.
#[derive(Debug)]
pub struct SearchActivation {
    /// What to show.
    pub request: SearchRequest,
    /// The compositor's activation context, when a valid token came with it.
    /// The desktop may send none yet; raise the window anyway.
    pub context: Option<ActivationContext>,
    /// A malformed token was discarded; the request still stands.
    pub context_error: Option<InboundTokenError>,
}

struct Queued {
    request: SearchRequest,
    token: Option<InboundActivationToken>,
    token_error: Option<InboundTokenError>,
    received_at: Instant,
}

#[derive(Default)]
struct Queue {
    calls: VecDeque<Queued>,
    refused: u64,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The user's choices from the desktop's Search, kept until the app drains
/// them. Cheap to clone.
#[derive(Clone, Default)]
pub struct SearchInbox {
    queue: Arc<Mutex<Queue>>,
}

impl std::fmt::Debug for SearchInbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let queue = lock(&self.queue);
        f.debug_struct("SearchInbox")
            .field("queued", &queue.calls.len())
            .field("refused", &queue.refused)
            .finish()
    }
}

impl SearchInbox {
    /// Takes at most `limit` activations and admits their tokens.
    pub fn drain(
        &self,
        broker: &mut ActivationBroker,
        now: Instant,
        limit: usize,
    ) -> Vec<SearchActivation> {
        let queued: Vec<Queued> = {
            let mut queue = lock(&self.queue);
            let count = limit.min(queue.calls.len());
            queue.calls.drain(..count).collect()
        };
        queued
            .into_iter()
            .map(|call| {
                let (context, context_error) = super::admit_queued_context(
                    broker,
                    call.token,
                    call.token_error,
                    call.received_at,
                    now,
                );
                SearchActivation {
                    request: call.request,
                    context,
                    context_error,
                }
            })
            .collect()
    }

    /// Activations refused because the app had not drained the inbox.
    #[must_use]
    pub fn refused(&self) -> u64 {
        lock(&self.queue).refused
    }
}

/// Results answered recently, for desktops that ask for a result's details
/// (GNOME) or open it (KRunner) by id alone.
const REMEMBERED_RESULTS: usize = 4 * MAX_SEARCH_RESULTS;

/// The search shared by every interface the provider is served over.
pub(super) struct SearchCore {
    provider: SearchProvider,
    inbox: SearchInbox,
    completion: CompletionSignal,
    // The newest search per caller; an older one still running gives way.
    latest: Mutex<HashMap<String, u64>>,
    next: std::sync::atomic::AtomicU64,
    superseded: tokio::sync::Notify,
    // Newest last: each result with the query that found it.
    recent: Mutex<VecDeque<(String, SearchResult)>>,
}

impl SearchCore {
    /// The core and the inbox its activations go to.
    pub(super) fn new(
        provider: SearchProvider,
        completion: CompletionSignal,
    ) -> (Arc<Self>, SearchInbox) {
        let inbox = SearchInbox::default();
        (
            Arc::new(Self {
                provider,
                inbox: inbox.clone(),
                completion,
                latest: Mutex::default(),
                next: std::sync::atomic::AtomicU64::new(1),
                superseded: tokio::sync::Notify::new(),
                recent: Mutex::default(),
            }),
            inbox,
        )
    }

    /// Runs the provider for `caller`, giving way to a newer query from the
    /// same caller and to the deadline, and remembers what it answered.
    async fn search(
        &self,
        caller: String,
        query: String,
        limit: usize,
        locale: Option<String>,
    ) -> Vec<SearchResult> {
        if query.trim().is_empty() || query.len() > MAX_QUERY_BYTES {
            return Vec::new();
        }
        let generation = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        lock(&self.latest).insert(caller.clone(), generation);
        self.superseded.notify_waiters();

        let search = (self.provider.run)(SearchQuery {
            text: query.clone(),
            limit,
            locale,
        });
        let mut work = pin!(tokio::time::timeout(SEARCH_DEADLINE, search));
        let mut results = loop {
            let notified = self.superseded.notified();
            if lock(&self.latest).get(&caller) != Some(&generation) {
                break Vec::new();
            }
            match future::select(work.as_mut(), pin!(notified)).await {
                Either::Left((answer, _)) => break answer.unwrap_or_default(),
                Either::Right(((), _)) => {}
            }
        };
        {
            let mut latest = lock(&self.latest);
            if latest.get(&caller) == Some(&generation) {
                latest.remove(&caller);
            }
        }
        results.truncate(limit);
        let mut recent = lock(&self.recent);
        for result in &results {
            recent.retain(|(_, kept)| kept.id != result.id);
            recent.push_back((query.clone(), result.clone()));
        }
        while recent.len() > REMEMBERED_RESULTS {
            recent.pop_front();
        }
        results
    }

    /// A result answered recently, with the query that found it.
    fn remembered(&self, id: &str) -> Option<(String, SearchResult)> {
        lock(&self.recent)
            .iter()
            .rev()
            .find(|(_, result)| result.id == id)
            .cloned()
    }

    fn enqueue(
        &self,
        request: SearchRequest,
        options: &HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<()> {
        let text_ok = match &request {
            SearchRequest::Open { id, query } => {
                !id.is_empty() && id.len() <= MAX_RESULT_ID_BYTES && query.len() <= MAX_QUERY_BYTES
            }
            SearchRequest::Search { query } => query.len() <= MAX_QUERY_BYTES,
        };
        if !text_ok {
            return Err(zbus::fdo::Error::InvalidArgs(
                "search activation exceeds its bound".into(),
            ));
        }
        let (token, token_error) = match crate::activation::inbound_platform_data(options) {
            Ok(token) => (token, None),
            Err(error) => (None, Some(error)),
        };
        let mut queue = lock(&self.inbox.queue);
        if queue.calls.len() >= SEARCH_ACTIVATION_CAPACITY {
            queue.refused = queue.refused.saturating_add(1);
            return Err(zbus::fdo::Error::LimitsExceeded(
                "search activation queue is full".into(),
            ));
        }
        queue.calls.push_back(Queued {
            request,
            token,
            token_error,
            received_at: Instant::now(),
        });
        drop(queue);
        self.completion.notify();
        Ok(())
    }
}

fn caller(header: &zbus::message::Header<'_>) -> String {
    header
        .sender()
        .map_or_else(String::new, ToString::to_string)
}

/// `dev.enzui.SearchProvider1`, enzUI's own desktop Search.
pub(super) struct SearchObject(pub(super) Arc<SearchCore>);

#[zbus::interface(name = "dev.enzui.SearchProvider1")]
impl SearchObject {
    #[zbus(name = "Search")]
    async fn search(
        &self,
        query: String,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> Vec<HashMap<String, OwnedValue>> {
        let limit = options
            .get("limit")
            .and_then(|value| u32::try_from(value).ok())
            .map_or(DEFAULT_SEARCH_LIMIT, |limit| limit as usize)
            .min(MAX_SEARCH_RESULTS);
        let locale = options
            .get("locale")
            .and_then(|value| value.try_clone().ok())
            .and_then(|value| String::try_from(value).ok());
        self.0
            .search(caller(&header), query, limit, locale)
            .await
            .iter()
            .map(SearchResult::to_wire)
            .collect()
    }

    #[zbus(name = "Activate")]
    fn activate(
        &self,
        id: String,
        query: String,
        options: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<()> {
        self.0.enqueue(SearchRequest::Open { id, query }, &options)
    }

    #[zbus(name = "LaunchSearch")]
    fn launch_search(
        &self,
        query: String,
        options: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<()> {
        self.0.enqueue(SearchRequest::Search { query }, &options)
    }

    #[zbus(property, name = "Version")]
    fn version(&self) -> u32 {
        1
    }
}

/// `org.gnome.Shell.SearchProvider2`: GNOME Shell's overview search.
pub(super) struct GnomeSearchObject(pub(super) Arc<SearchCore>);

impl GnomeSearchObject {
    async fn ids(&self, terms: Vec<String>, header: &zbus::message::Header<'_>) -> Vec<String> {
        self.0
            .search(caller(header), terms.join(" "), DEFAULT_SEARCH_LIMIT, None)
            .await
            .into_iter()
            .map(|result| result.id)
            .collect()
    }
}

#[zbus::interface(name = "org.gnome.Shell.SearchProvider2")]
impl GnomeSearchObject {
    async fn get_initial_result_set(
        &self,
        terms: Vec<String>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> Vec<String> {
        self.ids(terms, &header).await
    }

    /// The shell narrows its last answer as the user types; the provider
    /// is cheap enough to ask again.
    async fn get_subsearch_result_set(
        &self,
        _previous_results: Vec<String>,
        terms: Vec<String>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> Vec<String> {
        self.ids(terms, &header).await
    }

    fn get_result_metas(&self, identifiers: Vec<String>) -> Vec<HashMap<String, OwnedValue>> {
        identifiers
            .iter()
            .filter_map(|id| self.0.remembered(id))
            .map(|(_, result)| {
                let mut meta = HashMap::new();
                let mut put = |key: &str, value: Value<'_>| {
                    if let Ok(value) = value.try_to_owned() {
                        meta.insert(key.to_owned(), value);
                    }
                };
                put("id", Value::from(result.id.as_str()));
                put("name", Value::from(result.title.as_str()));
                if let Some(subtitle) = &result.subtitle {
                    put("description", Value::from(subtitle.as_str()));
                }
                if let Some(icon) = &result.icon {
                    put("gicon", Value::from(icon.as_str()));
                }
                meta
            })
            .collect()
    }

    fn activate_result(
        &self,
        identifier: String,
        terms: Vec<String>,
        _timestamp: u32,
    ) -> zbus::fdo::Result<()> {
        self.0.enqueue(
            SearchRequest::Open {
                id: identifier,
                query: terms.join(" "),
            },
            &HashMap::new(),
        )
    }

    fn launch_search(&self, terms: Vec<String>, _timestamp: u32) -> zbus::fdo::Result<()> {
        self.0.enqueue(
            SearchRequest::Search {
                query: terms.join(" "),
            },
            &HashMap::new(),
        )
    }
}

/// KRunner's match type for a result: exact for a near-certain hit.
fn krunner_type(score: Option<f64>) -> i32 {
    // Plasma::QueryMatch: PossibleMatch 30, ExactMatch 100.
    if score.is_some_and(|score| score >= 0.9) {
        100
    } else {
        30
    }
}

/// A KRunner match: id, text, icon, type, relevance and properties.
type KRunnerMatch = (
    String,
    String,
    String,
    i32,
    f64,
    HashMap<String, OwnedValue>,
);

/// `org.kde.krunner1`: KDE Plasma's KRunner and launcher search.
pub(super) struct KRunnerObject(pub(super) Arc<SearchCore>);

#[zbus::interface(name = "org.kde.krunner1")]
impl KRunnerObject {
    async fn r#match(
        &self,
        query: String,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> Vec<KRunnerMatch> {
        self.0
            .search(caller(&header), query, MAX_SEARCH_RESULTS, None)
            .await
            .into_iter()
            .map(|result| {
                let mut properties = HashMap::new();
                if let Some(subtitle) = &result.subtitle
                    && let Ok(value) = Value::from(subtitle.as_str()).try_to_owned()
                {
                    properties.insert("subtext".to_owned(), value);
                }
                (
                    result.id.clone(),
                    result.title.clone(),
                    result.icon.clone().unwrap_or_default(),
                    krunner_type(result.score),
                    result.score.unwrap_or(0.5),
                    properties,
                )
            })
            .collect()
    }

    fn actions(&self) -> Vec<(String, String, String)> {
        Vec::new()
    }

    fn run(&self, match_id: String, _action_id: String) -> zbus::fdo::Result<()> {
        let query = self.0.remembered(&match_id).map(|(query, _)| query);
        self.0.enqueue(
            SearchRequest::Open {
                id: match_id,
                query: query.unwrap_or_default(),
            },
            &HashMap::new(),
        )
    }

    fn teardown(&self) {}
}

/// The declaration the desktop finds this app's provider by, to install as
/// `$XDG_DATA_DIRS/enzui/search-providers/<app_id>.ini`. It names the bus
/// name and object path [`ApplicationService`](super::ApplicationService)
/// serves, so the two cannot drift. `min_query_length` is the shortest query
/// worth asking about.
#[must_use]
pub fn search_provider_ini(app_id: &str, min_query_length: Option<u32>) -> String {
    let mut ini = format!(
        "[enzUI Search Provider]\nDesktopId={app_id}.desktop\nBusName={app_id}\nObjectPath={}\nVersion=1\n",
        application_object_path(app_id)
    );
    if let Some(length) = min_query_length {
        let _ = writeln!(ini, "MinQueryLength={length}");
    }
    ini
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_result_refuses_what_the_desktop_would_drop() {
        assert_eq!(SearchResult::new("", "Title"), Err(SearchResultError::Id));
        assert_eq!(
            SearchResult::new("x".repeat(MAX_RESULT_ID_BYTES + 1), "Title"),
            Err(SearchResultError::Id)
        );
        assert!(SearchResult::new("x".repeat(MAX_RESULT_ID_BYTES), "Title").is_ok());
        assert_eq!(SearchResult::new("a", "  "), Err(SearchResultError::Title));
        assert_eq!(
            SearchResult::new("a", "bell\u{7}"),
            Err(SearchResultError::Title)
        );
        assert!(SearchResult::new("a", "two\tcolumns").is_ok());
    }
}
