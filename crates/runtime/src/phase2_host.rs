//! Generated Phase 2 import host backed by the UAPI dispatcher.
//!
//! This is the first real wiring from Wasmtime-generated traits into the
//! runtime dispatcher. Path-level filesystem calls, HTTP, time, locale, log, and
//! stdio handles flow through UCap before reaching a host adapter.

use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

use crate::{
    phase2_bindings::krate::{fs, io, locale, net, random, resources, store, time},
    phase2_bridge as bridge,
    uapi::UapiGuard,
    uapi_dispatch::{FileHandle, HostAdapter, UapiDispatcher},
};

use wasmtime::component::Resource;

const MAX_PHASE2_HOST_RESOURCES: usize = 1024;
const DEFAULT_HTTP_CLIENT_GET_TIMEOUT_MILLIS: u32 = 5_000;
const MAX_BUNDLED_RESOURCE_BYTES: u64 = 64 * 1024 * 1024;

pub struct Phase2Host<'a> {
    guard: UapiGuard,
    adapter: Box<dyn HostAdapter + 'a>,
    resources: Phase2ResourceTable,
    asset_root: Option<PathBuf>,
    default_http_timeout_millis: Option<u32>,
    /// The app's own key-value store. `None` until a run supplies a location,
    /// in which case every store call refuses -- an app cannot conjure storage
    /// the runtime did not give it.
    store: Option<crate::store_host::AppStore>,
    /// The app's own database, on the same terms as the store above.
    database: Option<crate::sql_host::AppDatabase>,
    /// The app's own secrets, encrypted at rest.
    secrets: Option<crate::secret_host::AppSecrets>,
    /// The app's shared store: a bucket synced between the machines that
    /// hold its invite code. `None` unless the run granted `store.shared`.
    shared: Option<crate::shared_host::AppShared>,
    /// Files the person chose in a dialog this run.
    ///
    /// Shared with the GUI host, which is what shows the dialog: the picker
    /// puts a path in, and `open_chosen` takes it out. Empty for a CLI app,
    /// which has no window to show a dialog from.
    chosen_files: std::rc::Rc<std::cell::RefCell<crate::chosen_files::ChosenFiles>>,
    /// Whether this run may draw random bytes.
    ///
    /// A plain flag rather than an `Option` holding state: entropy comes from
    /// the OS on every call, so there is nothing to open, nothing to keep, and
    /// nothing to close.
    random_granted: bool,
    /// Requests started with `http-client.begin` and not yet answered.
    ///
    /// Lives here so it dies with the run: a handle cannot outlive the app
    /// that made it, the same way a chosen-file token cannot.
    async_fetches: crate::async_fetch::AsyncFetches,
    async_ws: crate::async_ws::AsyncWs,
}

impl<'a> Phase2Host<'a> {
    pub fn new(guard: UapiGuard, adapter: Box<dyn HostAdapter + 'a>) -> Self {
        Self::new_with_http_timeout(guard, adapter, Some(DEFAULT_HTTP_CLIENT_GET_TIMEOUT_MILLIS))
    }

    pub fn new_with_http_timeout(
        guard: UapiGuard,
        adapter: Box<dyn HostAdapter + 'a>,
        default_http_timeout_millis: Option<u32>,
    ) -> Self {
        Self {
            guard,
            adapter,
            resources: Phase2ResourceTable::default(),
            asset_root: None,
            store: None,
            database: None,
            secrets: None,
            shared: None,
            chosen_files: Default::default(),
            random_granted: false,
            async_fetches: crate::async_fetch::AsyncFetches::new(),
            async_ws: crate::async_ws::AsyncWs::new(),
            default_http_timeout_millis,
        }
    }

    /// Test-only view of the registry, so the K-083 lock can prove both
    /// hosts hold the same one without making the field public.
    #[cfg(test)]
    pub(crate) fn chosen_files_for_test(
        &self,
    ) -> &std::rc::Rc<std::cell::RefCell<crate::chosen_files::ChosenFiles>> {
        &self.chosen_files
    }

    /// Share the chosen-files registry with the dialog host.
    ///
    /// Both hosts used to build their own registry independently, so a token
    /// the phase-3 picker issued resolved against an always-empty map here
    /// and every `fs.open-chosen` in a GUI app answered NotFound (K-083).
    /// The comment on the picker claimed the two were shared; this is what
    /// actually shares them.
    pub fn with_chosen_files(
        mut self,
        chosen: std::rc::Rc<std::cell::RefCell<crate::chosen_files::ChosenFiles>>,
    ) -> Self {
        self.chosen_files = chosen;
        self
    }

    pub fn with_asset_root(mut self, asset_root: Option<PathBuf>) -> Self {
        self.asset_root = asset_root;
        self
    }

    /// Say whether this run may draw random bytes.
    ///
    /// Resolved from the session policy like every other grant, so an app that
    /// was refused `random.bytes` is told `Denied` rather than quietly handed
    /// something weaker.
    pub fn with_random(mut self, granted: bool) -> Self {
        self.random_granted = granted;
        self
    }

    /// Give this run its key-value store.
    ///
    /// `granted` comes from the resolved session policy rather than from the
    /// app, so an app that was refused `store.kv` gets a store that answers
    /// `Denied` to everything instead of no store at all -- the refusal is
    /// explicit rather than looking like a missing feature.
    pub fn with_store(mut self, path: Option<PathBuf>, granted: bool) -> Self {
        self.store = path.map(|path| crate::store_host::AppStore::open(path, granted));
        self
    }

    /// Give this run its database, on the same terms as the key-value store:
    /// the grant is resolved once from the session policy, and the file is not
    /// opened until the app actually runs a statement.
    pub fn with_database(mut self, path: Option<PathBuf>, granted: bool) -> Self {
        self.database = path.map(|path| crate::sql_host::AppDatabase::new(path, granted));
        self
    }

    /// Give this run its secret store. The machine key never reaches the app;
    /// it only ever derives the key the runtime encrypts with.
    pub fn with_secrets(
        mut self,
        path: Option<PathBuf>,
        app_id: &str,
        machine_key: &[u8],
        granted: bool,
    ) -> Self {
        self.secrets = path
            .map(|path| crate::secret_host::AppSecrets::open(path, app_id, machine_key, granted));
        self
    }

    /// Give this run its shared store. `granted` resolves the capability
    /// once, like every other grant; without it every call answers Denied.
    pub fn with_shared(mut self, path: Option<PathBuf>, hub: String, granted: bool) -> Self {
        self.shared = match (path, granted) {
            (Some(path), true) => Some(crate::shared_host::AppShared::open(path, hub)),
            _ => None,
        };
        self
    }

    fn dispatcher(&self) -> UapiDispatcher<'_> {
        UapiDispatcher::new(&self.guard, self.adapter.as_ref())
    }
}

impl io::types::Host for Phase2Host<'_> {}
impl io::streams::Host for Phase2Host<'_> {}
impl io::args::Host for Phase2Host<'_> {
    fn raw(&mut self) -> wasmtime::Result<String> {
        self.dispatcher()
            .args_raw()
            .map_err(bridge::dispatch_error_to_trap)
    }
}
impl fs::types::Host for Phase2Host<'_> {}
impl net::types::Host for Phase2Host<'_> {}
impl locale::types::Host for Phase2Host<'_> {}

impl resources::assets::Host for Phase2Host<'_> {
    fn read(
        &mut self,
        path: String,
    ) -> wasmtime::Result<Result<Vec<u8>, resources::assets::ResourceError>> {
        let Some(root) = self.asset_root.as_deref() else {
            return Ok(Err(resources::assets::ResourceError::NotFound));
        };
        let path = match bundled_resource_path(root, &path) {
            Ok(path) => path,
            Err(error) => return Ok(Err(error)),
        };
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => return Ok(Err(resources::assets::ResourceError::NotFound)),
            Err(error) => return Ok(Err(resource_io_error(error))),
        };
        if metadata.len() > MAX_BUNDLED_RESOURCE_BYTES {
            return Ok(Err(resources::assets::ResourceError::TooLarge));
        }
        Ok(std::fs::read(path).map_err(resource_io_error))
    }

    fn list(
        &mut self,
        path: String,
    ) -> wasmtime::Result<Result<Vec<String>, resources::assets::ResourceError>> {
        let Some(root) = self.asset_root.as_deref() else {
            return Ok(Err(resources::assets::ResourceError::NotFound));
        };
        let path = match bundled_resource_path_allow_root(root, &path) {
            Ok(path) => path,
            Err(error) => return Ok(Err(error)),
        };
        let entries = match std::fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) => return Ok(Err(resource_io_error(error))),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => return Ok(Err(resource_io_error(error))),
            };
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                return Ok(Err(resources::assets::ResourceError::InvalidPath));
            };
            names.push(name);
        }
        names.sort();
        Ok(Ok(names))
    }
}

impl store::kv::Host for Phase2Host<'_> {
    fn get(
        &mut self,
        key: String,
    ) -> wasmtime::Result<Result<Option<Vec<u8>>, store::kv::StoreError>> {
        Ok(match self.store.as_ref() {
            Some(store) => store.get(&key).map_err(store_error_to_wit),
            None => Err(store::kv::StoreError::Denied),
        })
    }

    fn set(
        &mut self,
        key: String,
        value: Vec<u8>,
    ) -> wasmtime::Result<Result<(), store::kv::StoreError>> {
        Ok(match self.store.as_mut() {
            Some(store) => store.set(&key, value).map_err(store_error_to_wit),
            None => Err(store::kv::StoreError::Denied),
        })
    }

    fn delete(&mut self, key: String) -> wasmtime::Result<Result<(), store::kv::StoreError>> {
        Ok(match self.store.as_mut() {
            Some(store) => store.delete(&key).map_err(store_error_to_wit),
            None => Err(store::kv::StoreError::Denied),
        })
    }

    fn keys(&mut self) -> wasmtime::Result<Result<Vec<String>, store::kv::StoreError>> {
        Ok(match self.store.as_ref() {
            Some(store) => store.keys().map_err(store_error_to_wit),
            None => Err(store::kv::StoreError::Denied),
        })
    }

    fn clear(&mut self) -> wasmtime::Result<Result<(), store::kv::StoreError>> {
        Ok(match self.store.as_mut() {
            Some(store) => store.clear().map_err(store_error_to_wit),
            None => Err(store::kv::StoreError::Denied),
        })
    }
}

impl random::bytes::Host for Phase2Host<'_> {
    fn get(&mut self, count: u32) -> wasmtime::Result<Result<Vec<u8>, random::bytes::RandomError>> {
        if !self.random_granted {
            return Ok(Err(random::bytes::RandomError::Denied));
        }
        Ok(crate::random_host::bytes(count).map_err(random_error_to_wit))
    }

    fn next_u64(&mut self) -> wasmtime::Result<Result<u64, random::bytes::RandomError>> {
        if !self.random_granted {
            return Ok(Err(random::bytes::RandomError::Denied));
        }
        Ok(crate::random_host::next_u64().map_err(random_error_to_wit))
    }

    fn below(&mut self, bound: u64) -> wasmtime::Result<Result<u64, random::bytes::RandomError>> {
        if !self.random_granted {
            return Ok(Err(random::bytes::RandomError::Denied));
        }
        Ok(match crate::random_host::below(bound) {
            Ok(Some(value)) => Ok(value),
            // A bound of zero names an empty range, so there is no value to
            // return. Reported as such rather than silently as zero, which is
            // indistinguishable from a legitimate draw.
            Ok(None) => Err(random::bytes::RandomError::EmptyRange),
            Err(err) => Err(random_error_to_wit(err)),
        })
    }
}

fn random_error_to_wit(err: crate::random_host::RandomError) -> random::bytes::RandomError {
    use crate::random_host::RandomError;
    match err {
        RandomError::Denied => random::bytes::RandomError::Denied,
        RandomError::TooLarge => random::bytes::RandomError::TooLarge,
        RandomError::Unavailable(why) => random::bytes::RandomError::Unavailable(why),
    }
}

impl store::sql::Host for Phase2Host<'_> {
    fn query(
        &mut self,
        statement: String,
        params: Vec<store::sql::Value>,
    ) -> wasmtime::Result<Result<store::sql::QueryResult, store::sql::SqlError>> {
        let Some(db) = self.database.as_mut() else {
            return Ok(Err(store::sql::SqlError::Denied));
        };
        let params: Vec<_> = params.into_iter().map(value_from_wit).collect();
        Ok(db
            .query(&statement, &params)
            .map(|result| store::sql::QueryResult {
                columns: result.columns,
                rows: result
                    .rows
                    .into_iter()
                    .map(|values| store::sql::Row {
                        values: values.into_iter().map(value_to_wit).collect(),
                    })
                    .collect(),
            })
            .map_err(sql_error_to_wit))
    }

    fn execute(
        &mut self,
        statement: String,
        params: Vec<store::sql::Value>,
    ) -> wasmtime::Result<Result<u64, store::sql::SqlError>> {
        let Some(db) = self.database.as_mut() else {
            return Ok(Err(store::sql::SqlError::Denied));
        };
        let params: Vec<_> = params.into_iter().map(value_from_wit).collect();
        Ok(db.execute(&statement, &params).map_err(sql_error_to_wit))
    }

    fn transaction(
        &mut self,
        statements: Vec<String>,
    ) -> wasmtime::Result<Result<(), store::sql::SqlError>> {
        let Some(db) = self.database.as_mut() else {
            return Ok(Err(store::sql::SqlError::Denied));
        };
        Ok(db.transaction(&statements).map_err(sql_error_to_wit))
    }
}

impl store::secret::Host for Phase2Host<'_> {
    fn get(
        &mut self,
        name: String,
    ) -> wasmtime::Result<Result<Option<Vec<u8>>, store::secret::SecretError>> {
        Ok(match self.secrets.as_ref() {
            Some(secrets) => secrets.get(&name).map_err(secret_error_to_wit),
            None => Err(store::secret::SecretError::Denied),
        })
    }

    fn set(
        &mut self,
        name: String,
        secret: Vec<u8>,
    ) -> wasmtime::Result<Result<(), store::secret::SecretError>> {
        Ok(match self.secrets.as_mut() {
            Some(secrets) => secrets.set(&name, secret).map_err(secret_error_to_wit),
            None => Err(store::secret::SecretError::Denied),
        })
    }

    fn delete(&mut self, name: String) -> wasmtime::Result<Result<(), store::secret::SecretError>> {
        Ok(match self.secrets.as_mut() {
            Some(secrets) => secrets.delete(&name).map_err(secret_error_to_wit),
            None => Err(store::secret::SecretError::Denied),
        })
    }

    fn names(&mut self) -> wasmtime::Result<Result<Vec<String>, store::secret::SecretError>> {
        Ok(match self.secrets.as_ref() {
            Some(secrets) => secrets.names().map_err(secret_error_to_wit),
            None => Err(store::secret::SecretError::Denied),
        })
    }
}

impl store::shared::Host for Phase2Host<'_> {
    fn code(&mut self) -> wasmtime::Result<Result<Option<String>, store::shared::SharedError>> {
        Ok(match self.shared.as_ref() {
            Some(shared) => Ok(shared.code()),
            None => Err(store::shared::SharedError::Denied),
        })
    }

    fn create(&mut self) -> wasmtime::Result<Result<String, store::shared::SharedError>> {
        Ok(match self.shared.as_mut() {
            Some(shared) => shared.create().map_err(shared_error_to_wit),
            None => Err(store::shared::SharedError::Denied),
        })
    }

    fn join(&mut self, code: String) -> wasmtime::Result<Result<(), store::shared::SharedError>> {
        Ok(match self.shared.as_mut() {
            Some(shared) => shared.join(&code).map_err(shared_error_to_wit),
            None => Err(store::shared::SharedError::Denied),
        })
    }

    fn leave(&mut self) -> wasmtime::Result<Result<(), store::shared::SharedError>> {
        Ok(match self.shared.as_mut() {
            Some(shared) => shared.leave().map_err(shared_error_to_wit),
            None => Err(store::shared::SharedError::Denied),
        })
    }

    fn get(
        &mut self,
        key: String,
    ) -> wasmtime::Result<Result<Option<Vec<u8>>, store::shared::SharedError>> {
        Ok(match self.shared.as_ref() {
            Some(shared) => shared.get(&key).map_err(shared_error_to_wit),
            None => Err(store::shared::SharedError::Denied),
        })
    }

    fn set(
        &mut self,
        key: String,
        value: Vec<u8>,
    ) -> wasmtime::Result<Result<(), store::shared::SharedError>> {
        Ok(match self.shared.as_mut() {
            Some(shared) => shared.set(&key, value).map_err(shared_error_to_wit),
            None => Err(store::shared::SharedError::Denied),
        })
    }

    fn delete(&mut self, key: String) -> wasmtime::Result<Result<(), store::shared::SharedError>> {
        Ok(match self.shared.as_mut() {
            Some(shared) => shared.delete(&key).map_err(shared_error_to_wit),
            None => Err(store::shared::SharedError::Denied),
        })
    }

    fn keys(&mut self) -> wasmtime::Result<Result<Vec<String>, store::shared::SharedError>> {
        Ok(match self.shared.as_ref() {
            Some(shared) => Ok(shared.keys()),
            None => Err(store::shared::SharedError::Denied),
        })
    }

    fn sync(&mut self) -> wasmtime::Result<Result<bool, store::shared::SharedError>> {
        Ok(match self.shared.as_mut() {
            Some(shared) => shared.sync().map_err(shared_error_to_wit),
            None => Err(store::shared::SharedError::Denied),
        })
    }
}

fn shared_error_to_wit(error: crate::shared_host::SharedError) -> store::shared::SharedError {
    use crate::shared_host::SharedError;
    match error {
        SharedError::Denied => store::shared::SharedError::Denied,
        SharedError::NotJoined => store::shared::SharedError::NotJoined,
        SharedError::NoSuchShare => store::shared::SharedError::NoSuchShare,
        SharedError::InvalidName => store::shared::SharedError::InvalidName,
        SharedError::TooLarge => store::shared::SharedError::TooLarge,
        SharedError::Io(message) => store::shared::SharedError::Io(message),
    }
}

fn secret_error_to_wit(error: crate::secret_host::SecretError) -> store::secret::SecretError {
    use crate::secret_host::SecretError;
    match error {
        SecretError::Denied => store::secret::SecretError::Denied,
        SecretError::InvalidName => store::secret::SecretError::InvalidName,
        SecretError::TooLarge => store::secret::SecretError::TooLarge,
        SecretError::Io(message) => store::secret::SecretError::Io(message),
    }
}

fn value_from_wit(value: store::sql::Value) -> crate::sql_host::SqlValue {
    use crate::sql_host::SqlValue;
    match value {
        store::sql::Value::Null => SqlValue::Null,
        store::sql::Value::Integer(n) => SqlValue::Integer(n),
        store::sql::Value::Real(n) => SqlValue::Real(n),
        store::sql::Value::Text(text) => SqlValue::Text(text),
        store::sql::Value::Blob(bytes) => SqlValue::Blob(bytes),
    }
}

fn value_to_wit(value: crate::sql_host::SqlValue) -> store::sql::Value {
    use crate::sql_host::SqlValue;
    match value {
        SqlValue::Null => store::sql::Value::Null,
        SqlValue::Integer(n) => store::sql::Value::Integer(n),
        SqlValue::Real(n) => store::sql::Value::Real(n),
        SqlValue::Text(text) => store::sql::Value::Text(text),
        SqlValue::Blob(bytes) => store::sql::Value::Blob(bytes),
    }
}

fn sql_error_to_wit(error: crate::sql_host::SqlError) -> store::sql::SqlError {
    use crate::sql_host::SqlError;
    match error {
        SqlError::Denied => store::sql::SqlError::Denied,
        SqlError::InvalidStatement(message) => store::sql::SqlError::InvalidStatement(message),
        SqlError::Forbidden(message) => store::sql::SqlError::Forbidden(message),
        SqlError::TooLarge => store::sql::SqlError::TooLarge,
        SqlError::Io(message) => store::sql::SqlError::Io(message),
        // A host with no database at all -- a browser preview. The WIT has
        // no variant for this and must not grow one: the guest contract is
        // what every existing .krate was compiled against, and an app that
        // handles Io already handles this. The words carry the truth, so
        // whoever reads them knows it is the place, not the app.
        SqlError::Unsupported => store::sql::SqlError::Io(
            "this app keeps records on your computer, which a browser preview cannot do -- \
             download it to use this part"
                .to_string(),
        ),
    }
}

fn store_error_to_wit(error: crate::store_host::StoreError) -> store::kv::StoreError {
    use crate::store_host::StoreError;
    match error {
        StoreError::Denied => store::kv::StoreError::Denied,
        StoreError::InvalidKey => store::kv::StoreError::InvalidKey,
        StoreError::TooLarge => store::kv::StoreError::TooLarge,
        StoreError::Io(message) => store::kv::StoreError::Io(message),
    }
}

fn bundled_resource_path(
    root: &Path,
    requested: &str,
) -> Result<PathBuf, resources::assets::ResourceError> {
    if requested.is_empty() {
        return Err(resources::assets::ResourceError::InvalidPath);
    }
    bundled_resource_path_allow_root(root, requested)
}

fn bundled_resource_path_allow_root(
    root: &Path,
    requested: &str,
) -> Result<PathBuf, resources::assets::ResourceError> {
    if requested.contains('\\') || requested.chars().any(char::is_control) {
        return Err(resources::assets::ResourceError::InvalidPath);
    }
    let requested = Path::new(requested);
    if requested.is_absolute()
        || requested
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
            && !requested.as_os_str().is_empty()
    {
        return Err(resources::assets::ResourceError::InvalidPath);
    }
    Ok(root.join(requested))
}

fn resource_io_error(error: std::io::Error) -> resources::assets::ResourceError {
    match error.kind() {
        std::io::ErrorKind::NotFound => resources::assets::ResourceError::NotFound,
        _ => resources::assets::ResourceError::Io(error.to_string()),
    }
}

impl fs::files::Host for Phase2Host<'_> {
    fn open(
        &mut self,
        path: String,
        mode: fs::types::OpenMode,
    ) -> wasmtime::Result<Result<Resource<fs::files::File>, fs::types::FsError>> {
        let mode = bridge::open_mode_from_wit(mode);
        let handle = match self.dispatcher().fs_open(&path, mode) {
            Ok(handle) => handle,
            Err(err) => return Ok(Err(bridge::fs_error_to_wit(err))),
        };

        Ok(Ok(self.resources.insert_file(handle)?))
    }

    fn open_chosen(
        &mut self,
        token: String,
        mode: fs::types::OpenMode,
    ) -> wasmtime::Result<Result<Resource<fs::files::File>, fs::types::FsError>> {
        // The token is the only way in. An app cannot name a path here, so it
        // cannot open a file it was never handed -- and a token from a previous
        // run resolves to nothing, because the store lives and dies with the
        // run that issued it.
        let Some(path) = self
            .chosen_files
            .borrow()
            .resolve(&token)
            .map(Path::to_path_buf)
        else {
            return Ok(Err(fs::types::FsError::NotFound));
        };
        let mode = bridge::open_mode_from_wit(mode);
        let handle = match self.dispatcher().fs_open_chosen(&path, mode) {
            Ok(handle) => handle,
            Err(err) => return Ok(Err(bridge::fs_error_to_wit(err))),
        };
        Ok(Ok(self.resources.insert_file(handle)?))
    }

    fn stat(
        &mut self,
        path: String,
    ) -> wasmtime::Result<Result<fs::types::FileStat, fs::types::FsError>> {
        Ok(self
            .dispatcher()
            .fs_stat(&path)
            .map(bridge::file_stat_to_wit)
            .map_err(bridge::fs_error_to_wit))
    }

    fn list(&mut self, path: String) -> wasmtime::Result<Result<Vec<String>, fs::types::FsError>> {
        Ok(self
            .dispatcher()
            .fs_list(&path)
            .map_err(bridge::fs_error_to_wit))
    }

    fn remove_file(&mut self, path: String) -> wasmtime::Result<Result<(), fs::types::FsError>> {
        Ok(self
            .dispatcher()
            .fs_remove_file(&path)
            .map_err(bridge::fs_error_to_wit))
    }

    fn remove_dir(&mut self, path: String) -> wasmtime::Result<Result<(), fs::types::FsError>> {
        Ok(self
            .dispatcher()
            .fs_remove_dir(&path)
            .map_err(bridge::fs_error_to_wit))
    }

    fn mkdir(&mut self, path: String) -> wasmtime::Result<Result<(), fs::types::FsError>> {
        Ok(self
            .dispatcher()
            .fs_mkdir(&path)
            .map_err(bridge::fs_error_to_wit))
    }

    fn rename(
        &mut self,
        from: String,
        to: String,
    ) -> wasmtime::Result<Result<(), fs::types::FsError>> {
        Ok(self
            .dispatcher()
            .fs_rename(&from, &to)
            .map_err(bridge::fs_error_to_wit))
    }
}

impl fs::files::HostFile for Phase2Host<'_> {
    fn read(
        &mut self,
        self_: Resource<fs::files::File>,
        n: u32,
    ) -> wasmtime::Result<Result<Vec<u8>, fs::types::FsError>> {
        let Some(handle) = self.resources.file(self_.rep()).cloned() else {
            return Ok(Err(missing_file_resource()));
        };

        Ok(self
            .dispatcher()
            .fs_read(&handle, n)
            .map_err(bridge::fs_error_to_wit))
    }

    fn write(
        &mut self,
        self_: Resource<fs::files::File>,
        bytes: Vec<u8>,
    ) -> wasmtime::Result<Result<u32, fs::types::FsError>> {
        let Some(handle) = self.resources.file(self_.rep()).cloned() else {
            return Ok(Err(missing_file_resource()));
        };

        Ok(self
            .dispatcher()
            .fs_write(&handle, &bytes)
            .map_err(bridge::fs_error_to_wit))
    }

    fn seek_set(
        &mut self,
        self_: Resource<fs::files::File>,
        pos: u64,
    ) -> wasmtime::Result<Result<u64, fs::types::FsError>> {
        let Some(handle) = self.resources.file(self_.rep()).cloned() else {
            return Ok(Err(missing_file_resource()));
        };

        Ok(self
            .dispatcher()
            .fs_seek_set(&handle, pos)
            .map_err(bridge::fs_error_to_wit))
    }

    fn seek_end(
        &mut self,
        self_: Resource<fs::files::File>,
    ) -> wasmtime::Result<Result<u64, fs::types::FsError>> {
        let Some(handle) = self.resources.file(self_.rep()).cloned() else {
            return Ok(Err(missing_file_resource()));
        };

        Ok(self
            .dispatcher()
            .fs_seek_end(&handle)
            .map_err(bridge::fs_error_to_wit))
    }

    fn stat(
        &mut self,
        self_: Resource<fs::files::File>,
    ) -> wasmtime::Result<Result<fs::types::FileStat, fs::types::FsError>> {
        let Some(handle) = self.resources.file(self_.rep()).cloned() else {
            return Ok(Err(missing_file_resource()));
        };

        Ok(self
            .dispatcher()
            .fs_stat_handle(&handle)
            .map(bridge::file_stat_to_wit)
            .map_err(bridge::fs_error_to_wit))
    }

    fn drop(&mut self, rep: Resource<fs::files::File>) -> wasmtime::Result<()> {
        if let Some(handle) = self.resources.file(rep.rep()).cloned() {
            let _ = self.dispatcher().close_file_handle(&handle);
        }
        self.resources.remove_file(rep.rep());
        Ok(())
    }
}

impl io::stdio::Host for Phase2Host<'_> {
    fn stdin(&mut self) -> wasmtime::Result<Resource<io::streams::InputStream>> {
        let handle = self
            .dispatcher()
            .stdin()
            .map_err(bridge::dispatch_error_to_trap)?;
        self.resources.insert_input(handle)
    }

    fn stdout(&mut self) -> wasmtime::Result<Resource<io::streams::OutputStream>> {
        let handle = self
            .dispatcher()
            .stdout()
            .map_err(bridge::dispatch_error_to_trap)?;
        self.resources.insert_output(handle)
    }

    fn stderr(&mut self) -> wasmtime::Result<Resource<io::streams::OutputStream>> {
        let handle = self
            .dispatcher()
            .stderr()
            .map_err(bridge::dispatch_error_to_trap)?;
        self.resources.insert_output(handle)
    }
}

impl io::streams::HostInputStream for Phase2Host<'_> {
    fn read(
        &mut self,
        self_: Resource<io::streams::InputStream>,
        n: u32,
    ) -> wasmtime::Result<Result<Vec<u8>, io::types::IoError>> {
        let Some(handle) = self.resources.input(self_.rep()).cloned() else {
            return Ok(Err(missing_stream_resource()));
        };

        Ok(self
            .dispatcher()
            .read_stream(&handle, n)
            .map_err(bridge::io_error_to_wit))
    }

    fn read_to_string(
        &mut self,
        self_: Resource<io::streams::InputStream>,
    ) -> wasmtime::Result<Result<String, io::types::IoError>> {
        let Some(handle) = self.resources.input(self_.rep()).cloned() else {
            return Ok(Err(missing_stream_resource()));
        };

        Ok(self
            .dispatcher()
            .read_stream_to_string(&handle)
            .map_err(bridge::io_error_to_wit))
    }

    fn drop(&mut self, rep: Resource<io::streams::InputStream>) -> wasmtime::Result<()> {
        if let Some(handle) = self.resources.input(rep.rep()).cloned() {
            let _ = self.dispatcher().close_stream_handle(&handle);
        }
        self.resources.remove_input(rep.rep());
        Ok(())
    }
}

impl io::streams::HostOutputStream for Phase2Host<'_> {
    fn write(
        &mut self,
        self_: Resource<io::streams::OutputStream>,
        bytes: Vec<u8>,
    ) -> wasmtime::Result<Result<u32, io::types::IoError>> {
        let Some(handle) = self.resources.output(self_.rep()).cloned() else {
            return Ok(Err(missing_stream_resource()));
        };

        Ok(self
            .dispatcher()
            .write_stream(&handle, &bytes)
            .map_err(bridge::io_error_to_wit))
    }

    fn write_all(
        &mut self,
        self_: Resource<io::streams::OutputStream>,
        bytes: Vec<u8>,
    ) -> wasmtime::Result<Result<(), io::types::IoError>> {
        let Some(handle) = self.resources.output(self_.rep()).cloned() else {
            return Ok(Err(missing_stream_resource()));
        };

        Ok(self
            .dispatcher()
            .write_all_stream(&handle, &bytes)
            .map_err(bridge::io_error_to_wit))
    }

    fn flush(
        &mut self,
        self_: Resource<io::streams::OutputStream>,
    ) -> wasmtime::Result<Result<(), io::types::IoError>> {
        let Some(handle) = self.resources.output(self_.rep()).cloned() else {
            return Ok(Err(missing_stream_resource()));
        };

        Ok(self
            .dispatcher()
            .flush_stream(&handle)
            .map_err(bridge::io_error_to_wit))
    }

    fn drop(&mut self, rep: Resource<io::streams::OutputStream>) -> wasmtime::Result<()> {
        if let Some(handle) = self.resources.output(rep.rep()).cloned() {
            let _ = self.dispatcher().close_stream_handle(&handle);
        }
        self.resources.remove_output(rep.rep());
        Ok(())
    }
}

impl io::log::Host for Phase2Host<'_> {
    fn emit(
        &mut self,
        level: io::types::LogLevel,
        message: String,
        _fields: Vec<io::log::Field>,
    ) -> wasmtime::Result<()> {
        self.dispatcher()
            .log(bridge::log_level_to_str(level), &message)
            .map_err(bridge::dispatch_error_to_trap)
    }
}

impl net::http_client::Host for Phase2Host<'_> {
    fn get(&mut self, url: String) -> wasmtime::Result<Result<Vec<u8>, net::types::NetError>> {
        let req = crate::uapi_dispatch::HttpRequest {
            method: crate::uapi_dispatch::HttpMethod::Get,
            url,
            headers: Vec::new(),
            body: Vec::new(),
            timeout_millis: self.default_http_timeout_millis,
        };

        Ok(self
            .dispatcher()
            .net_fetch(req)
            .map(|response| response.body)
            .map_err(bridge::net_error_to_wit))
    }

    fn fetch(
        &mut self,
        req: net::types::Request,
    ) -> wasmtime::Result<Result<net::types::Response, net::types::NetError>> {
        Ok(self
            .dispatcher()
            .net_fetch(bridge::request_from_wit(req))
            .map(bridge::response_to_wit)
            .map_err(bridge::net_error_to_wit))
    }

    fn begin(
        &mut self,
        req: net::types::Request,
    ) -> wasmtime::Result<Result<u64, net::types::NetError>> {
        // The grant is checked here, before any worker exists, so a handle is
        // only ever issued for a host the person allowed.
        let job = match self
            .dispatcher()
            .net_fetch_job(bridge::request_from_wit(req))
        {
            Ok(Some(job)) => job,
            // The grant passed but this adapter has no off-thread path. Say so
            // rather than falling back to a blocking fetch, which would freeze
            // the app -- the exact thing this call exists to avoid.
            Ok(None) => {
                return Ok(Err(net::types::NetError::Other(
                    "this host cannot run a request in the background".to_string(),
                )))
            }
            Err(err) => return Ok(Err(bridge::net_error_to_wit(err))),
        };
        // A refusal here is an ordinary answer, not a failure of the runtime:
        // too many requests are already in the air. Reporting it lets the app
        // wait and retry, where the alternative was the host running out of
        // OS threads and panicking (K-137).
        Ok(match self.async_fetches.begin(job) {
            Ok(handle) => Ok(handle),
            Err(err) => Err(bridge::net_error_to_wit(err.into())),
        })
    }

    fn poll(&mut self, handle: u64) -> wasmtime::Result<net::types::FetchStatus> {
        use crate::async_fetch::FetchStatus;
        Ok(match self.async_fetches.poll(handle) {
            FetchStatus::Pending => net::types::FetchStatus::Pending,
            FetchStatus::Ready(response) => {
                net::types::FetchStatus::Ready(bridge::response_to_wit(response))
            }
            FetchStatus::Failed(err) => {
                net::types::FetchStatus::Failed(bridge::net_error_to_wit(err.into()))
            }
            FetchStatus::UnknownHandle => net::types::FetchStatus::UnknownHandle,
        })
    }

    fn cancel(&mut self, handle: u64) -> wasmtime::Result<()> {
        self.async_fetches.cancel(handle);
        Ok(())
    }
}

impl net::ws::Host for Phase2Host<'_> {
    fn open(&mut self, url: String) -> wasmtime::Result<Result<u64, net::types::NetError>> {
        // The grant is checked here, before any worker exists, exactly as
        // for a fetch: same wall, same wording, same refusal shape.
        if let Err(err) = self.dispatcher().check_ws_url(&url) {
            return Ok(Err(bridge::net_error_to_wit(err)));
        }
        Ok(self.async_ws.open(url).map_err(net::types::NetError::Other))
    }

    fn send(
        &mut self,
        handle: u64,
        message: net::ws::WsMessage,
    ) -> wasmtime::Result<Result<(), net::types::NetError>> {
        let message = match message {
            net::ws::WsMessage::Text(text) => crate::async_ws::WsMessage::Text(text),
            net::ws::WsMessage::Binary(bytes) => crate::async_ws::WsMessage::Binary(bytes),
        };
        Ok(self
            .async_ws
            .send(handle, message)
            .map_err(net::types::NetError::Other))
    }

    fn poll(&mut self, handle: u64) -> wasmtime::Result<net::ws::WsEvent> {
        use crate::async_ws::{WsEvent, WsMessage};
        Ok(match self.async_ws.poll(handle) {
            WsEvent::Pending => net::ws::WsEvent::Pending,
            WsEvent::Opened => net::ws::WsEvent::Opened,
            WsEvent::Message(WsMessage::Text(text)) => {
                net::ws::WsEvent::Message(net::ws::WsMessage::Text(text))
            }
            WsEvent::Message(WsMessage::Binary(bytes)) => {
                net::ws::WsEvent::Message(net::ws::WsMessage::Binary(bytes))
            }
            WsEvent::Closed => net::ws::WsEvent::Closed,
            WsEvent::Failed(reason) => net::ws::WsEvent::Failed(reason),
            WsEvent::UnknownHandle => net::ws::WsEvent::UnknownHandle,
        })
    }

    fn close(&mut self, handle: u64) -> wasmtime::Result<()> {
        self.async_ws.close(handle);
        Ok(())
    }
}

impl time::clock::Host for Phase2Host<'_> {
    fn now_millis(&mut self) -> wasmtime::Result<u64> {
        self.dispatcher()
            .now_millis()
            .map_err(bridge::dispatch_error_to_trap)
    }

    fn monotonic_nanos(&mut self) -> wasmtime::Result<u64> {
        self.dispatcher()
            .monotonic_nanos()
            .map_err(bridge::dispatch_error_to_trap)
    }
}

impl time::sleep::Host for Phase2Host<'_> {
    fn sleep_millis(&mut self, millis: u32) -> wasmtime::Result<()> {
        self.dispatcher()
            .sleep_millis(millis)
            .map_err(bridge::dispatch_error_to_trap)
    }
}

impl locale::info::Host for Phase2Host<'_> {
    fn current(&mut self) -> wasmtime::Result<locale::types::LocaleId> {
        self.dispatcher()
            .current_locale()
            .map(bridge::locale_to_wit)
            .map_err(bridge::dispatch_error_to_trap)
    }

    fn timezone(&mut self) -> wasmtime::Result<String> {
        self.dispatcher()
            .timezone()
            .map_err(bridge::dispatch_error_to_trap)
    }
}

impl locale::format::Host for Phase2Host<'_> {
    fn format_date(
        &mut self,
        millis: u64,
        tz: String,
        style: locale::types::DateStyle,
        loc: locale::types::LocaleId,
    ) -> wasmtime::Result<String> {
        let loc = bridge::locale_from_wit(loc);
        self.dispatcher()
            .format_date(millis, &tz, bridge::date_style_from_wit(style), &loc)
            .map_err(bridge::dispatch_error_to_trap)
    }

    fn format_number(
        &mut self,
        value: f64,
        style: locale::types::NumberStyle,
        loc: locale::types::LocaleId,
    ) -> wasmtime::Result<String> {
        let loc = bridge::locale_from_wit(loc);
        self.dispatcher()
            .format_number(value, bridge::number_style_from_wit(style), &loc)
            .map_err(bridge::dispatch_error_to_trap)
    }
}

#[derive(Default)]
struct Phase2ResourceTable {
    next_id: u32,
    free_ids: Vec<u32>,
    files: BTreeMap<u32, FileHandle>,
    inputs: BTreeMap<u32, FileHandle>,
    outputs: BTreeMap<u32, FileHandle>,
}

impl Phase2ResourceTable {
    fn insert_file(&mut self, handle: FileHandle) -> wasmtime::Result<Resource<fs::files::File>> {
        self.ensure_capacity()?;
        let id = self.allocate_id()?;
        self.files.insert(id, handle);
        Ok(Resource::new_own(id))
    }

    fn insert_input(
        &mut self,
        handle: FileHandle,
    ) -> wasmtime::Result<Resource<io::streams::InputStream>> {
        self.ensure_capacity()?;
        let id = self.allocate_id()?;
        self.inputs.insert(id, handle);
        Ok(Resource::new_own(id))
    }

    fn insert_output(
        &mut self,
        handle: FileHandle,
    ) -> wasmtime::Result<Resource<io::streams::OutputStream>> {
        self.ensure_capacity()?;
        let id = self.allocate_id()?;
        self.outputs.insert(id, handle);
        Ok(Resource::new_own(id))
    }

    fn file(&self, id: u32) -> Option<&FileHandle> {
        self.files.get(&id)
    }

    fn input(&self, id: u32) -> Option<&FileHandle> {
        self.inputs.get(&id)
    }

    fn output(&self, id: u32) -> Option<&FileHandle> {
        self.outputs.get(&id)
    }

    fn remove_file(&mut self, id: u32) {
        if self.files.remove(&id).is_some() {
            self.release_id(id);
        }
    }

    fn remove_input(&mut self, id: u32) {
        if self.inputs.remove(&id).is_some() {
            self.release_id(id);
        }
    }

    fn remove_output(&mut self, id: u32) {
        if self.outputs.remove(&id).is_some() {
            self.release_id(id);
        }
    }

    fn ensure_capacity(&self) -> wasmtime::Result<()> {
        let total = self.files.len() + self.inputs.len() + self.outputs.len();
        if total >= MAX_PHASE2_HOST_RESOURCES {
            return Err(wasmtime::Error::msg(format!(
                "Phase 2 host resource table exceeds limit ({MAX_PHASE2_HOST_RESOURCES})"
            )));
        }
        Ok(())
    }

    fn allocate_id(&mut self) -> wasmtime::Result<u32> {
        match self.free_ids.pop() {
            Some(id) => Ok(id),
            None => {
                let id = self.next_id;
                self.next_id = self
                    .next_id
                    .checked_add(1)
                    .ok_or_else(|| wasmtime::Error::msg("Phase 2 resource table exhausted"))?;
                Ok(id)
            }
        }
    }

    fn release_id(&mut self, id: u32) {
        self.free_ids.push(id);
    }
}

fn missing_file_resource() -> fs::types::FsError {
    fs::types::FsError::Io("unknown file resource".to_string())
}

fn missing_stream_resource() -> io::types::IoError {
    io::types::IoError::Other("unknown stream resource".to_string())
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use krate_policy::SessionPolicy;

    use crate::uapi_dispatch::{
        AdapterError, DateStyle, FsAdapter, Header, HttpRequest, HttpResponse, IoAdapter,
        LocaleAdapter, LocaleId, NetAdapter, OpenMode, TimeAdapter,
    };

    use super::*;

    #[derive(Clone, Default)]
    struct RecordingAdapter {
        calls: Rc<RefCell<Calls>>,
        net_error: Option<AdapterError>,
    }

    impl RecordingAdapter {
        fn with_net_error(error: AdapterError) -> Self {
            Self {
                calls: Rc::default(),
                net_error: Some(error),
            }
        }
    }

    #[derive(Default)]
    struct Calls {
        close_file: usize,
        close_stream: usize,
        file_read: usize,
        fs_stat: usize,
        last_timeout_millis: Option<Option<u32>>,
        log: usize,
        net_fetch: usize,
        sleep: usize,
        stream_write_all: usize,
    }

    impl HostAdapter for RecordingAdapter {
        fn io(&self) -> &dyn IoAdapter {
            self
        }

        fn fs(&self) -> &dyn FsAdapter {
            self
        }

        fn net(&self) -> &dyn NetAdapter {
            self
        }

        fn time(&self) -> &dyn TimeAdapter {
            self
        }

        fn locale(&self) -> &dyn LocaleAdapter {
            self
        }
    }

    impl IoAdapter for RecordingAdapter {
        fn stdin(&self) -> Result<FileHandle, AdapterError> {
            Ok(FileHandle::resource(10))
        }

        fn stdout(&self) -> Result<FileHandle, AdapterError> {
            Ok(FileHandle::resource(11))
        }

        fn stderr(&self) -> Result<FileHandle, AdapterError> {
            Ok(FileHandle::resource(12))
        }

        fn args_raw(&self) -> Result<String, AdapterError> {
            Ok("fixtures/a.txt".to_string())
        }

        fn read_stream(&self, handle: &FileHandle, _n: u32) -> Result<Vec<u8>, AdapterError> {
            Ok(format!("stream-{}", handle.id).into_bytes())
        }

        fn read_stream_to_string(&self, handle: &FileHandle) -> Result<String, AdapterError> {
            Ok(format!("stream-{}", handle.id))
        }

        fn write_stream(&self, _handle: &FileHandle, bytes: &[u8]) -> Result<u32, AdapterError> {
            Ok(bytes.len() as u32)
        }

        fn write_all_stream(
            &self,
            _handle: &FileHandle,
            _bytes: &[u8],
        ) -> Result<(), AdapterError> {
            self.calls.borrow_mut().stream_write_all += 1;
            Ok(())
        }

        fn flush_stream(&self, _handle: &FileHandle) -> Result<(), AdapterError> {
            Ok(())
        }

        fn close_stream(&self, _handle: &FileHandle) -> Result<(), AdapterError> {
            self.calls.borrow_mut().close_stream += 1;
            Ok(())
        }

        fn log(&self, _level: &str, _message: &str) -> Result<(), AdapterError> {
            self.calls.borrow_mut().log += 1;
            Ok(())
        }
    }

    impl FsAdapter for RecordingAdapter {
        fn open(&self, _path: &str, _mode: OpenMode) -> Result<FileHandle, AdapterError> {
            Ok(FileHandle::resource(20))
        }

        fn read(&self, handle: &FileHandle, _n: u32) -> Result<Vec<u8>, AdapterError> {
            self.calls.borrow_mut().file_read += 1;
            Ok(format!("file-{}", handle.id).into_bytes())
        }

        fn write(&self, _handle: &FileHandle, bytes: &[u8]) -> Result<u32, AdapterError> {
            Ok(bytes.len() as u32)
        }

        fn seek_set(&self, _handle: &FileHandle, pos: u64) -> Result<u64, AdapterError> {
            Ok(pos)
        }

        fn seek_end(&self, _handle: &FileHandle) -> Result<u64, AdapterError> {
            Ok(7)
        }

        fn stat_handle(
            &self,
            _handle: &FileHandle,
        ) -> Result<crate::uapi_dispatch::FileStat, AdapterError> {
            Ok(crate::uapi_dispatch::FileStat {
                size: 7,
                modified_millis: 5678,
                is_dir: false,
            })
        }

        fn stat(&self, _path: &str) -> Result<crate::uapi_dispatch::FileStat, AdapterError> {
            self.calls.borrow_mut().fs_stat += 1;
            Ok(crate::uapi_dispatch::FileStat {
                size: 64,
                modified_millis: 1234,
                is_dir: false,
            })
        }

        fn list(&self, _path: &str) -> Result<Vec<String>, AdapterError> {
            Ok(vec!["one.txt".to_string()])
        }

        fn remove_file(&self, _path: &str) -> Result<(), AdapterError> {
            Ok(())
        }

        fn remove_dir(&self, _path: &str) -> Result<(), AdapterError> {
            Ok(())
        }

        fn mkdir(&self, _path: &str) -> Result<(), AdapterError> {
            Ok(())
        }

        fn rename(&self, _from: &str, _to: &str) -> Result<(), AdapterError> {
            Ok(())
        }

        fn close_file(&self, _handle: &FileHandle) -> Result<(), AdapterError> {
            self.calls.borrow_mut().close_file += 1;
            Ok(())
        }
    }

    impl NetAdapter for RecordingAdapter {
        fn fetch(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
            let mut calls = self.calls.borrow_mut();
            calls.net_fetch += 1;
            calls.last_timeout_millis = Some(req.timeout_millis);
            if let Some(error) = &self.net_error {
                return Err(error.clone());
            }
            Ok(HttpResponse {
                status: 200,
                headers: vec![Header {
                    name: "x-url".to_string(),
                    value: req.url,
                }],
                body: b"ok".to_vec(),
            })
        }
    }

    impl TimeAdapter for RecordingAdapter {
        fn now_millis(&self) -> Result<u64, AdapterError> {
            Ok(100)
        }

        fn monotonic_nanos(&self) -> Result<u64, AdapterError> {
            Ok(200)
        }

        fn sleep_millis(&self, _millis: u32) -> Result<(), AdapterError> {
            self.calls.borrow_mut().sleep += 1;
            Ok(())
        }
    }

    impl LocaleAdapter for RecordingAdapter {
        fn current(&self) -> Result<LocaleId, AdapterError> {
            Ok(LocaleId {
                bcp47: "en-US".to_string(),
            })
        }

        fn timezone(&self) -> Result<String, AdapterError> {
            Ok("UTC".to_string())
        }

        fn format_date(
            &self,
            millis: u64,
            tz: &str,
            style: DateStyle,
            loc: &LocaleId,
        ) -> Result<String, AdapterError> {
            Ok(format!("{millis}:{tz}:{style:?}:{}", loc.bcp47))
        }

        fn format_number(
            &self,
            value: f64,
            style: crate::uapi_dispatch::NumberStyle,
            loc: &LocaleId,
        ) -> Result<String, AdapterError> {
            Ok(format!("{value}:{style:?}:{}", loc.bcp47))
        }
    }

    #[test]
    fn generated_get_uses_configured_default_timeout() {
        let adapter = RecordingAdapter::default();
        let guard = UapiGuard::new(SessionPolicy::from_grants(["net.connect:example.com:80"
            .parse()
            .unwrap()]));
        let mut host =
            Phase2Host::new_with_http_timeout(guard, Box::new(adapter.clone()), Some(2500));

        let response =
            net::http_client::Host::get(&mut host, "http://example.com/path".to_string())
                .unwrap()
                .unwrap();

        assert_eq!(response, b"ok".to_vec());
        assert_eq!(adapter.calls.borrow().last_timeout_millis, Some(Some(2500)));
    }

    #[test]
    fn a_ws_open_without_the_grant_is_refused_before_any_socket() {
        let adapter = RecordingAdapter::default();
        let guard = UapiGuard::new(SessionPolicy::from_grants([]));
        let mut host = Phase2Host::new(guard, Box::new(adapter));

        let refused = net::ws::Host::open(&mut host, "ws://example.com:9001".to_string())
            .unwrap()
            .unwrap_err();
        assert!(
            matches!(refused, net::types::NetError::PermissionDenied),
            "expected permission-denied, got {refused:?}"
        );
    }

    #[test]
    fn a_ws_open_with_the_grant_issues_a_handle_and_close_is_safe() {
        let adapter = RecordingAdapter::default();
        let guard = UapiGuard::new(SessionPolicy::from_grants(["net.connect:localhost:1"
            .parse()
            .unwrap()]));
        let mut host = Phase2Host::new(guard, Box::new(adapter));

        // The wall passes, so a handle is issued; the dial itself fails on
        // the worker (nothing listens on port 1) and reports through poll.
        let handle = net::ws::Host::open(&mut host, "ws://localhost:1".to_string())
            .unwrap()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match net::ws::Host::poll(&mut host, handle).unwrap() {
                net::ws::WsEvent::Failed(_) => break,
                net::ws::WsEvent::Pending if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(10))
                }
                other => panic!("expected the dial to fail, got {other:?}"),
            }
        }
        // A retired handle answers unknown-handle, and close on it is safe.
        assert!(matches!(
            net::ws::Host::poll(&mut host, handle).unwrap(),
            net::ws::WsEvent::UnknownHandle
        ));
        net::ws::Host::close(&mut host, handle).unwrap();
    }

    #[test]
    fn generated_get_can_disable_default_timeout() {
        let adapter = RecordingAdapter::default();
        let guard = UapiGuard::new(SessionPolicy::from_grants(["net.connect:example.com:80"
            .parse()
            .unwrap()]));
        let mut host = Phase2Host::new_with_http_timeout(guard, Box::new(adapter.clone()), None);

        let response =
            net::http_client::Host::get(&mut host, "http://example.com/path".to_string())
                .unwrap()
                .unwrap();

        assert_eq!(response, b"ok".to_vec());
        assert_eq!(adapter.calls.borrow().last_timeout_millis, Some(None));
    }

    #[test]
    fn generated_net_host_calls_dispatcher() {
        let adapter = RecordingAdapter::default();
        let guard = UapiGuard::new(SessionPolicy::from_grants(["net.connect:example.com:443"
            .parse()
            .unwrap()]));
        let mut host = Phase2Host::new(guard, Box::new(adapter.clone()));

        let response = net::http_client::Host::fetch(
            &mut host,
            net::types::Request {
                method: net::types::HttpMethod::Get,
                url: "https://example.com/path".to_string(),
                headers: Vec::new(),
                body: Vec::new(),
                timeout_millis: Some(42),
            },
        )
        .unwrap()
        .unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(adapter.calls.borrow().net_fetch, 1);
        assert_eq!(adapter.calls.borrow().last_timeout_millis, Some(Some(42)));
    }

    #[test]
    fn generated_net_host_returns_wit_permission_error() {
        let adapter = RecordingAdapter::default();
        let guard = UapiGuard::new(SessionPolicy::default());
        let mut host = Phase2Host::new(guard, Box::new(adapter.clone()));

        let err = net::http_client::Host::fetch(
            &mut host,
            net::types::Request {
                method: net::types::HttpMethod::Get,
                url: "https://example.com/path".to_string(),
                headers: Vec::new(),
                body: Vec::new(),
                timeout_millis: None,
            },
        )
        .unwrap()
        .unwrap_err();

        assert!(matches!(err, net::types::NetError::PermissionDenied));
        assert_eq!(adapter.calls.borrow().net_fetch, 0);
    }

    #[test]
    fn generated_net_host_returns_wit_body_too_large_error() {
        let adapter = RecordingAdapter::with_net_error(AdapterError::BodyTooLarge);
        let guard = UapiGuard::new(SessionPolicy::from_grants(["net.connect:example.com:443"
            .parse()
            .unwrap()]));
        let mut host = Phase2Host::new(guard, Box::new(adapter.clone()));

        let err = net::http_client::Host::fetch(
            &mut host,
            net::types::Request {
                method: net::types::HttpMethod::Get,
                url: "https://example.com/path".to_string(),
                headers: Vec::new(),
                body: Vec::new(),
                timeout_millis: None,
            },
        )
        .unwrap()
        .unwrap_err();

        assert!(matches!(err, net::types::NetError::BodyTooLarge));
        assert_eq!(adapter.calls.borrow().net_fetch, 1);
    }

    #[test]
    fn generated_net_host_returns_wit_timeout_error() {
        let adapter = RecordingAdapter::with_net_error(AdapterError::Timeout);
        let guard = UapiGuard::new(SessionPolicy::from_grants(["net.connect:example.com:443"
            .parse()
            .unwrap()]));
        let mut host = Phase2Host::new(guard, Box::new(adapter.clone()));

        let err = net::http_client::Host::fetch(
            &mut host,
            net::types::Request {
                method: net::types::HttpMethod::Get,
                url: "https://example.com/path".to_string(),
                headers: Vec::new(),
                body: Vec::new(),
                timeout_millis: Some(1),
            },
        )
        .unwrap()
        .unwrap_err();

        assert!(matches!(err, net::types::NetError::Timeout));
        assert_eq!(adapter.calls.borrow().net_fetch, 1);
    }

    #[test]
    fn generated_net_host_returns_wit_protocol_error() {
        let adapter =
            RecordingAdapter::with_net_error(AdapterError::Protocol("bad status".to_string()));
        let guard = UapiGuard::new(SessionPolicy::from_grants(["net.connect:example.com:443"
            .parse()
            .unwrap()]));
        let mut host = Phase2Host::new(guard, Box::new(adapter.clone()));

        let err = net::http_client::Host::fetch(
            &mut host,
            net::types::Request {
                method: net::types::HttpMethod::Get,
                url: "https://example.com/path".to_string(),
                headers: Vec::new(),
                body: Vec::new(),
                timeout_millis: None,
            },
        )
        .unwrap()
        .unwrap_err();

        assert!(matches!(err, net::types::NetError::Protocol(message) if message == "bad status"));
        assert_eq!(adapter.calls.borrow().net_fetch, 1);
    }

    #[test]
    fn generated_fs_and_stdio_hosts_call_dispatcher() {
        let adapter = RecordingAdapter::default();
        let guard = UapiGuard::new(SessionPolicy::from_grants([
            "fs.read:/tmp/data.txt".parse().unwrap(),
            "io.stdin".parse().unwrap(),
        ]));
        let mut host = Phase2Host::new(guard, Box::new(adapter.clone()));

        let stat = fs::files::Host::stat(&mut host, "/tmp/data.txt".to_string())
            .unwrap()
            .unwrap();
        let stdin = io::stdio::Host::stdin(&mut host).unwrap();

        assert_eq!(stat.size, 64);
        assert_eq!(stdin.rep(), 0);
        assert_eq!(adapter.calls.borrow().fs_stat, 1);
    }

    #[test]
    fn generated_time_locale_and_log_hosts_call_dispatcher() {
        let adapter = RecordingAdapter::default();
        let guard = UapiGuard::new(SessionPolicy::default());
        let mut host = Phase2Host::new(guard, Box::new(adapter.clone()));

        assert_eq!(time::clock::Host::now_millis(&mut host).unwrap(), 100);
        time::sleep::Host::sleep_millis(&mut host, 1).unwrap();
        assert_eq!(
            locale::info::Host::current(&mut host).unwrap().bcp47,
            "en-US"
        );
        io::log::Host::emit(
            &mut host,
            io::types::LogLevel::Info,
            "hello".to_string(),
            Vec::new(),
        )
        .unwrap();

        assert_eq!(adapter.calls.borrow().sleep, 1);
        assert_eq!(adapter.calls.borrow().log, 1);
    }

    #[test]
    fn resource_table_routes_file_and_stream_operations() {
        let adapter = RecordingAdapter::default();
        let guard = UapiGuard::new(SessionPolicy::from_grants([
            "fs.read:/tmp/data.txt".parse().unwrap(),
            "io.stdout".parse().unwrap(),
        ]));
        let mut host = Phase2Host::new(guard, Box::new(adapter.clone()));

        let file = fs::files::Host::open(
            &mut host,
            "/tmp/data.txt".to_string(),
            fs::types::OpenMode::Read,
        )
        .unwrap()
        .unwrap();
        let stdout = io::stdio::Host::stdout(&mut host).unwrap();

        let bytes = fs::files::HostFile::read(&mut host, file, 128)
            .unwrap()
            .unwrap();
        io::streams::HostOutputStream::write_all(&mut host, stdout, b"hello".to_vec())
            .unwrap()
            .unwrap();

        assert_eq!(bytes, b"file-20");
        assert_eq!(adapter.calls.borrow().file_read, 1);
        assert_eq!(adapter.calls.borrow().stream_write_all, 1);
    }

    #[test]
    fn resource_drop_closes_underlying_handles() {
        let adapter = RecordingAdapter::default();
        let guard = UapiGuard::new(SessionPolicy::from_grants([
            "fs.read:/tmp/data.txt".parse().unwrap(),
            "io.stdout".parse().unwrap(),
        ]));
        let mut host = Phase2Host::new(guard, Box::new(adapter.clone()));

        let file = fs::files::Host::open(
            &mut host,
            "/tmp/data.txt".to_string(),
            fs::types::OpenMode::Read,
        )
        .unwrap()
        .unwrap();
        let stdout = io::stdio::Host::stdout(&mut host).unwrap();

        fs::files::HostFile::drop(&mut host, file).unwrap();
        io::streams::HostOutputStream::drop(&mut host, stdout).unwrap();

        let calls = adapter.calls.borrow();
        assert_eq!(calls.close_file, 1);
        assert_eq!(calls.close_stream, 1);
    }

    #[test]
    fn host_resource_table_rejects_overflow() {
        let adapter = RecordingAdapter::default();
        let guard = UapiGuard::new(SessionPolicy::from_grants(["io.stdout".parse().unwrap()]));
        let mut host = Phase2Host::new(guard, Box::new(adapter));

        for _ in 0..MAX_PHASE2_HOST_RESOURCES {
            io::stdio::Host::stdout(&mut host).expect("allocate output stream within limit");
        }

        let err = io::stdio::Host::stdout(&mut host)
            .expect_err("host resource-table overflow should be rejected");
        assert!(
            err.to_string()
                .contains("Phase 2 host resource table exceeds limit"),
            "unexpected host resource-table overflow error: {err}"
        );
    }

    #[test]
    fn host_resource_table_reuses_released_ids() {
        let adapter = RecordingAdapter::default();
        let guard = UapiGuard::new(SessionPolicy::from_grants(["io.stdout".parse().unwrap()]));
        let mut host = Phase2Host::new(guard, Box::new(adapter));

        let first = io::stdio::Host::stdout(&mut host).expect("first stdout stream");
        let second = io::stdio::Host::stdout(&mut host).expect("second stdout stream");
        let second_id = second.rep();
        assert!(
            second_id > first.rep(),
            "resource ids should increase while allocating fresh handles"
        );

        io::streams::HostOutputStream::drop(&mut host, second)
            .expect("dropping stream should release host resource id");

        let reopened =
            io::stdio::Host::stdout(&mut host).expect("stream after drop should allocate");
        assert_eq!(
            reopened.rep(),
            second_id,
            "host resource table should reuse released ids before allocating new ones"
        );
    }

    #[test]
    fn host_resource_table_uses_free_id_before_counter_overflow() {
        let adapter = RecordingAdapter::default();
        let guard = UapiGuard::new(SessionPolicy::from_grants(["io.stdout".parse().unwrap()]));
        let mut host = Phase2Host::new(guard, Box::new(adapter));

        host.resources.next_id = u32::MAX;
        host.resources.free_ids.push(11);

        let stdout = io::stdio::Host::stdout(&mut host)
            .expect("free-list id should be used before fresh id allocation");
        assert_eq!(stdout.rep(), 11);
    }

    #[test]
    fn unknown_resources_return_clear_errors() {
        let adapter = RecordingAdapter::default();
        let guard = UapiGuard::new(SessionPolicy::default());
        let mut host = Phase2Host::new(guard, Box::new(adapter.clone()));

        let err = io::streams::HostOutputStream::write_all(
            &mut host,
            Resource::new_own(99),
            b"hello".to_vec(),
        )
        .unwrap()
        .unwrap_err();

        assert!(
            matches!(err, io::types::IoError::Other(message) if message.contains("unknown stream"))
        );
    }

    #[test]
    fn bundled_resources_are_read_without_filesystem_authority() {
        let dir = tempfile::tempdir().expect("assets dir");
        std::fs::create_dir_all(dir.path().join("prompts")).expect("nested dir");
        std::fs::write(dir.path().join("prompts/welcome.txt"), b"hello from bundle")
            .expect("write asset");
        let mut host = Phase2Host::new(UapiGuard::default(), Box::new(RecordingAdapter::default()))
            .with_asset_root(Some(dir.path().to_path_buf()));

        let bytes = resources::assets::Host::read(&mut host, "prompts/welcome.txt".to_string())
            .expect("host call")
            .expect("resource read");
        assert_eq!(bytes, b"hello from bundle");
    }

    #[test]
    fn bundled_resources_reject_paths_outside_the_asset_root() {
        let dir = tempfile::tempdir().expect("assets dir");
        let mut host = Phase2Host::new(UapiGuard::default(), Box::new(RecordingAdapter::default()))
            .with_asset_root(Some(dir.path().to_path_buf()));

        let error = resources::assets::Host::read(&mut host, "../secret".to_string())
            .expect("host call")
            .expect_err("parent traversal");
        assert!(matches!(
            error,
            resources::assets::ResourceError::InvalidPath
        ));
    }
}
