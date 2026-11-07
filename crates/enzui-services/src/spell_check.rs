//! Spell checking with the dictionaries the desktop has.
//!
//! [`open`] (or [`for_locale`]) gives the best checker for a language:
//! [`Enchant`], the desktop's spelling library, which reaches the Hunspell,
//! Nuspell, Aspell, Hspell and Voikko dictionaries installed; or, where
//! Enchant or its back ends are missing (a minimal system, a Flatpak
//! runtime), [`Hunspell`], which reads Hunspell dictionaries directly.
//! Both keep the user's added words in Enchant's personal word list
//! (`$XDG_CONFIG_HOME/enchant/<language>.dic`), shared with every Enchant
//! app. Enchant is loaded at first use, so a system without it still runs.
//!
//! ```no_run
//! # fn demo() -> Result<(), enzui_services::spell_check::SpellError> {
//! use enzui_core::spelling::SpellCheck;
//!
//! let check = SpellCheck::shared(enzui_services::spell_check::for_locale()?);
//! # Ok(()) }
//! ```

#![allow(unsafe_code, reason = "Enchant is loaded and called through FFI")]

use enzui_core::spelling::SpellChecker;
use libloading::Library;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

type Broker = *mut c_void;
type Dict = *mut c_void;
type BrokerInit = unsafe extern "C" fn() -> Broker;
type BrokerFree = unsafe extern "C" fn(Broker);
type RequestDict = unsafe extern "C" fn(Broker, *const c_char) -> Dict;
type FreeDict = unsafe extern "C" fn(Broker, Dict);
type DescribeFn =
    unsafe extern "C" fn(*const c_char, *const c_char, *const c_char, *const c_char, *mut c_void);
type ListDicts = unsafe extern "C" fn(Broker, DescribeFn, *mut c_void);
type DictCheck = unsafe extern "C" fn(Dict, *const c_char, isize) -> c_int;
type DictSuggest = unsafe extern "C" fn(Dict, *const c_char, isize, *mut usize) -> *mut *mut c_char;
type FreeStringList = unsafe extern "C" fn(Dict, *mut *mut c_char);
type DictAdd = unsafe extern "C" fn(Dict, *const c_char, isize);

/// Why spell checking is unavailable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpellError {
    /// Enchant is not installed; the text says what is missing.
    Unavailable(String),
    /// No dictionary for this language is installed.
    NoDictionary(String),
}

impl std::fmt::Display for SpellError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(reason) => write!(f, "spell checking is unavailable: {reason}"),
            Self::NoDictionary(language) => write!(f, "no {language} dictionary is installed"),
        }
    }
}

impl std::error::Error for SpellError {}

struct Api {
    broker_init: BrokerInit,
    broker_free: BrokerFree,
    request_dict: RequestDict,
    free_dict: FreeDict,
    list_dicts: ListDicts,
    check: DictCheck,
    suggest: DictSuggest,
    free_string_list: FreeStringList,
    add: DictAdd,
    _library: Library,
}

/// The process's Enchant, loaded on first use.
fn api() -> Result<&'static Api, SpellError> {
    static LOADED: std::sync::OnceLock<Result<Api, String>> = std::sync::OnceLock::new();
    LOADED
        .get_or_init(Api::load)
        .as_ref()
        .map_err(|reason| SpellError::Unavailable(reason.clone()))
}

impl Api {
    fn load() -> Result<Self, String> {
        // SAFETY: loading Enchant runs only its ELF initialisers, which have
        // no preconditions.
        let library = ["libenchant-2.so.2", "libenchant-2.so"]
            .into_iter()
            .find_map(|name| unsafe { Library::new(name) }.ok())
            .ok_or_else(|| "libenchant-2 is not installed".to_owned())?;
        macro_rules! symbol {
            ($name:literal) => {
                // SAFETY: each symbol is read with the signature Enchant 2
                // declares for it in enchant.h, and the copied function
                // pointer never outlives `_library`, which `Api` owns for the
                // life of the process.
                *unsafe { library.get(concat!($name, "\0").as_bytes()) }
                    .map_err(|_| format!("libenchant-2 lacks {}", $name))?
            };
        }
        Ok(Self {
            broker_init: symbol!("enchant_broker_init"),
            broker_free: symbol!("enchant_broker_free"),
            request_dict: symbol!("enchant_broker_request_dict"),
            free_dict: symbol!("enchant_broker_free_dict"),
            list_dicts: symbol!("enchant_broker_list_dicts"),
            check: symbol!("enchant_dict_check"),
            suggest: symbol!("enchant_dict_suggest"),
            free_string_list: symbol!("enchant_dict_free_string_list"),
            add: symbol!("enchant_dict_add"),
            _library: library,
        })
    }
}

/// An owned broker, freed on drop.
struct OwnedBroker {
    api: &'static Api,
    broker: NonNull<c_void>,
}

impl OwnedBroker {
    fn new(api: &'static Api) -> Result<Self, SpellError> {
        // SAFETY: `enchant_broker_init` has no preconditions.
        let broker = unsafe { (api.broker_init)() };
        NonNull::new(broker)
            .map(|broker| Self { api, broker })
            .ok_or_else(|| SpellError::Unavailable("Enchant did not start".into()))
    }
}

impl Drop for OwnedBroker {
    fn drop(&mut self) {
        // SAFETY: the broker came from `enchant_broker_init`, every dictionary
        // it handed out was freed first (`Inner::drop`), and it is freed once.
        unsafe { (self.api.broker_free)(self.broker.as_ptr()) };
    }
}

struct Inner {
    broker: OwnedBroker,
    dict: NonNull<c_void>,
}

// SAFETY: Enchant's broker and dictionary are plain heap objects with no
// thread affinity; `Enchant` only touches them behind its `Mutex`, one
// thread at a time.
unsafe impl Send for Inner {}

impl Drop for Inner {
    fn drop(&mut self) {
        // SAFETY: the dictionary came from this broker and is freed once,
        // before the broker.
        unsafe { (self.broker.api.free_dict)(self.broker.broker.as_ptr(), self.dict.as_ptr()) };
    }
}

/// A dictionary for one language, through Enchant.
pub struct Enchant {
    inner: Mutex<Inner>,
    language: String,
}

impl std::fmt::Debug for Enchant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Enchant")
            .field("language", &self.language)
            .finish_non_exhaustive()
    }
}

impl Enchant {
    /// The dictionary for `language`, a tag such as `en_US` or `de`.
    ///
    /// # Errors
    /// Enchant is not installed, or has no dictionary for `language`.
    pub fn open(language: &str) -> Result<Self, SpellError> {
        let api = api()?;
        let broker = OwnedBroker::new(api)?;
        let tag =
            CString::new(language).map_err(|_| SpellError::NoDictionary(language.to_owned()))?;
        // SAFETY: the broker is live and the tag is a NUL-terminated string
        // that outlives the call.
        let dict = unsafe { (api.request_dict)(broker.broker.as_ptr(), tag.as_ptr()) };
        let dict =
            NonNull::new(dict).ok_or_else(|| SpellError::NoDictionary(language.to_owned()))?;
        Ok(Self {
            inner: Mutex::new(Inner { broker, dict }),
            language: language.to_owned(),
        })
    }

    fn with<T>(&self, call: impl FnOnce(&'static Api, *mut c_void) -> T) -> T {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        call(inner.broker.api, inner.dict.as_ptr())
    }
}

/// The languages Enchant has dictionaries for, sorted.
///
/// # Errors
/// Enchant is not installed.
pub fn available_languages() -> Result<Vec<String>, SpellError> {
    unsafe extern "C" fn describe(
        tag: *const c_char,
        _provider: *const c_char,
        _description: *const c_char,
        _file: *const c_char,
        user: *mut c_void,
    ) {
        // SAFETY: `user` is the `Vec` `available_languages` passed, alive
        // and unaliased for the call; `tag` is a NUL-terminated string Enchant
        // owns for the call.
        unsafe {
            let tags = &mut *user.cast::<Vec<String>>();
            if !tag.is_null() {
                tags.push(CStr::from_ptr(tag).to_string_lossy().into_owned());
            }
        }
    }
    let api = api()?;
    let broker = OwnedBroker::new(api)?;
    let mut tags: Vec<String> = Vec::new();
    // SAFETY: the broker is live; the callback matches EnchantDictDescribeFn
    // and receives `tags`, which outlives this synchronous call.
    unsafe {
        (api.list_dicts)(
            broker.broker.as_ptr(),
            describe,
            (&raw mut tags).cast::<c_void>(),
        );
    }
    tags.sort();
    tags.dedup();
    Ok(tags)
}

/// `word`'s bytes and length for Enchant, which takes explicit lengths.
fn word_arg(word: &str) -> (*const c_char, isize) {
    (
        word.as_ptr().cast::<c_char>(),
        isize::try_from(word.len()).unwrap_or(isize::MAX),
    )
}

impl SpellChecker for Enchant {
    fn check(&self, word: &str) -> bool {
        if word.is_empty() {
            return true;
        }
        let (bytes, length) = word_arg(word);
        // SAFETY: the dictionary is live behind the lock; `bytes` points at
        // `length` bytes of UTF-8 that outlive the call.
        self.with(|api, dict| unsafe { (api.check)(dict, bytes, length) } == 0)
    }

    fn suggest(&self, word: &str) -> Vec<String> {
        if word.is_empty() {
            return Vec::new();
        }
        let (bytes, length) = word_arg(word);
        self.with(|api, dict| {
            let mut count = 0usize;
            // SAFETY: as in `check`; `count` receives the list's length.
            let list = unsafe { (api.suggest)(dict, bytes, length, &raw mut count) };
            if list.is_null() {
                return Vec::new();
            }
            let suggestions = (0..count)
                .filter_map(|index| {
                    // SAFETY: Enchant returned `count` entries, each a
                    // NUL-terminated string or NULL.
                    let entry = unsafe { *list.add(index) };
                    (!entry.is_null()).then(|| {
                        // SAFETY: a non-null entry is a NUL-terminated string
                        // the list owns until it is freed below.
                        unsafe { CStr::from_ptr(entry) }
                            .to_string_lossy()
                            .into_owned()
                    })
                })
                .collect();
            // SAFETY: the list came from `enchant_dict_suggest` on this
            // dictionary and is freed once.
            unsafe { (api.free_string_list)(dict, list) };
            suggestions
        })
    }

    fn add_to_dictionary(&self, word: &str) {
        if word.is_empty() {
            return;
        }
        let (bytes, length) = word_arg(word);
        // SAFETY: as in `check`.
        self.with(|api, dict| unsafe { (api.add)(dict, bytes, length) });
    }

    fn language(&self) -> &str {
        &self.language
    }
}

/// The best checker for `language` (a tag such as `en_US` or `de`):
/// [`Enchant`], else [`Hunspell`] over the standard dictionary directories.
///
/// # Errors
/// No dictionary for `language` is installed.
pub fn open(language: &str) -> Result<Arc<dyn SpellChecker>, SpellError> {
    match Enchant::open(language) {
        Ok(enchant) => Ok(Arc::new(enchant)),
        Err(_) => {
            Hunspell::open(language).map(|hunspell| Arc::new(hunspell) as Arc<dyn SpellChecker>)
        }
    }
}

/// The checker for the user's locale
/// ([`enzui_core::spelling::locale_language`]), or for its language alone
/// (`de` for `de_AT`), or English.
///
/// # Errors
/// None of these dictionaries is installed.
pub fn for_locale() -> Result<Arc<dyn SpellChecker>, SpellError> {
    let tag = enzui_core::spelling::locale_language();
    let language = tag.split('_').next().unwrap_or_default().to_owned();
    open(&tag)
        .or_else(|_| open(&language))
        .or_else(|_| open("en_US"))
        .or_else(|_| open("en"))
}

/// The directories Hunspell dictionaries are installed in: `hunspell` and
/// `myspell` (and `myspell/dicts`) under `$XDG_DATA_HOME` and each of
/// `$XDG_DATA_DIRS`.
#[must_use]
pub fn hunspell_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
    home.into_iter()
        .chain(
            system
                .split(':')
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
        )
        .flat_map(|dir| {
            [
                dir.join("hunspell"),
                dir.join("myspell"),
                dir.join("myspell/dicts"),
            ]
        })
        .collect()
}

/// Enchant's personal word list for `language`.
fn personal_list(language: &str) -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(config.join("enchant").join(format!("{language}.dic")))
}

/// A Hunspell dictionary read directly (`<language>.aff` and `.dic`), with
/// the user's personal word list.
pub struct Hunspell {
    dictionary: Mutex<spellbook::Dictionary>,
    language: String,
    personal: Option<PathBuf>,
}

impl std::fmt::Debug for Hunspell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hunspell")
            .field("language", &self.language)
            .finish_non_exhaustive()
    }
}

impl Hunspell {
    /// The dictionary for `language` from [`hunspell_dirs`], with Enchant's
    /// personal word list.
    ///
    /// # Errors
    /// No such dictionary is installed, or it does not parse.
    pub fn open(language: &str) -> Result<Self, SpellError> {
        Self::open_in(language, &hunspell_dirs(), personal_list(language))
    }

    /// The dictionary for `language` from the first of `dirs` that has it;
    /// added words go to `personal`, and the words already there are known.
    ///
    /// # Errors
    /// No such dictionary is in `dirs`, or it does not parse.
    pub fn open_in(
        language: &str,
        dirs: &[PathBuf],
        personal: Option<PathBuf>,
    ) -> Result<Self, SpellError> {
        let missing = || SpellError::NoDictionary(language.to_owned());
        let (aff, dic) = dirs
            .iter()
            .find_map(|dir| {
                let aff = std::fs::read_to_string(dir.join(format!("{language}.aff"))).ok()?;
                let dic = std::fs::read_to_string(dir.join(format!("{language}.dic"))).ok()?;
                Some((aff, dic))
            })
            .ok_or_else(missing)?;
        let mut dictionary = spellbook::Dictionary::new(&aff, &dic).map_err(|_| missing())?;
        if let Some(words) = personal
            .as_deref()
            .and_then(|path| std::fs::read_to_string(path).ok())
        {
            for word in words.lines().map(str::trim).filter(|word| !word.is_empty()) {
                let _ = dictionary.add(word);
            }
        }
        Ok(Self {
            dictionary: Mutex::new(dictionary),
            language: language.to_owned(),
            personal,
        })
    }

    fn dictionary(&self) -> std::sync::MutexGuard<'_, spellbook::Dictionary> {
        self.dictionary
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Appends `word` to the word list at `path`.
fn append_word(path: &Path, word: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{word}")
}

impl SpellChecker for Hunspell {
    fn check(&self, word: &str) -> bool {
        self.dictionary().check(word)
    }

    fn suggest(&self, word: &str) -> Vec<String> {
        let mut suggestions = Vec::new();
        self.dictionary().suggest(word, &mut suggestions);
        suggestions
    }

    fn add_to_dictionary(&self, word: &str) {
        let word = word.trim();
        if word.is_empty() || word.contains(['\n', '/']) {
            return;
        }
        let _ = self.dictionary().add(word);
        if let Some(path) = &self.personal
            && let Err(error) = append_word(path, word)
        {
            log::warn!(target: "enzui::spelling", "{}: {error}", path.display());
        }
    }

    fn language(&self) -> &str {
        &self.language
    }
}
