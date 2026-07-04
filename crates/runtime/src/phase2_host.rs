//! Generated Phase 2 import host backed by the UAPI dispatcher.
//!
//! This is the first real wiring from Wasmtime-generated traits into the
//! runtime dispatcher. Path-level filesystem calls, HTTP, time, locale, log, and
//! stdio handles flow through UCap before reaching a host adapter.

use std::collections::BTreeMap;

use crate::{
    phase2_bindings::krate::{fs, io, locale, net, time},
    phase2_bridge as bridge,
    uapi::UapiGuard,
    uapi_dispatch::{FileHandle, HostAdapter, UapiDispatcher},
};

use wasmtime::component::Resource;

const MAX_PHASE2_HOST_RESOURCES: usize = 1024;
const DEFAULT_HTTP_CLIENT_GET_TIMEOUT_MILLIS: u32 = 5_000;

pub struct Phase2Host<'a> {
    guard: UapiGuard,
    adapter: Box<dyn HostAdapter + 'a>,
    resources: Phase2ResourceTable,
    default_http_timeout_millis: Option<u32>,
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
            default_http_timeout_millis,
        }
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
}
