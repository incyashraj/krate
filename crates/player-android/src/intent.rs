//! Reading the launching intent through JNI.
//!
//! The player is pure native code, so the intent -- which .krate file or
//! link the person tapped -- is fetched from the Java side by hand:
//! android-activity fills ndk-context with the JavaVM and the activity
//! object, and everything else is plain method calls. Failures never
//! panic; a player that cannot read its intent shows Krate's home screen,
//! and the reason lands in logcat.

#![cfg(target_os = "android")]

use jni::objects::{JByteArray, JObject, JString, JValue};

/// The two raw handles android-activity exposes: the JavaVM and the
/// Activity's jobject. ndk-context is no substitute for the latter -- it
/// stores the Application, and Application has no getIntent.
#[derive(Clone, Copy)]
pub struct ActivityHandles {
    pub vm: *mut core::ffi::c_void,
    pub activity: *mut core::ffi::c_void,
}

/// The URI the activity was opened to VIEW -- at launch, or tapped while
/// Krate ran -- once: taking it clears it, so coming back to the home screen
/// does not open it again.
pub fn take_open(handles: ActivityHandles) -> Option<String> {
    let taken = (|| -> Result<Option<String>, String> {
        let vm = unsafe { jni::JavaVM::from_raw(handles.vm.cast()) }.map_err(err)?;
        let mut env = vm.attach_current_thread().map_err(err)?;
        let activity = unsafe { JObject::from_raw(handles.activity.cast()) };
        let open = env
            .call_method(&activity, "krateTakeOpen", "()Ljava/lang/String;", &[])
            .and_then(|v| v.l())
            .map_err(|e| {
                // The description goes to logcat as System.err before the
                // exception is cleared -- the only way to see which call blew.
                let _ = env.exception_describe();
                let _ = env.exception_clear();
                format!("krateTakeOpen: {e}")
            })?;
        Ok(get_string(&mut env, open))
    })();
    match taken {
        Ok(open) => open,
        Err(why) => {
            log::warn!("could not read the launch intent: {why}");
            None
        }
    }
}

/// Whether a link or file tapped while Krate's own screen showed is waiting
/// for [`take_open`].
pub fn open_waiting(handles: ActivityHandles) -> bool {
    (|| -> Result<bool, String> {
        let vm = unsafe { jni::JavaVM::from_raw(handles.vm.cast()) }.map_err(err)?;
        let mut env = vm.attach_current_thread().map_err(err)?;
        let activity = unsafe { JObject::from_raw(handles.activity.cast()) };
        env.call_method(&activity, "krateOpenWaiting", "()Z", &[])
            .and_then(|v| v.z())
            .map_err(err)
    })()
    .unwrap_or(false)
}

/// Read every byte behind a content:// or file:// URI through the
/// platform's ContentResolver -- the only path modern Android guarantees.
pub fn read_uri(handles: ActivityHandles, uri: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
    // A file:// link the player can read itself -- its own folder, or one a
    // file manager handed over with access -- is read directly. The
    // resolver only knows files the media index has seen, so a .krate saved
    // straight into the player's folder was refused as "could not be
    // opened" though it was right there.
    if let Some(path) = uri.strip_prefix("file://") {
        if let Err(e) = std::fs::metadata(path) {
            if e.kind() == std::io::ErrorKind::NotFound {
                return Err(
                    "The file is not there any more -- it may have been moved or deleted."
                        .to_string(),
                );
            }
        }
        if let Ok(meta) = std::fs::metadata(path) {
            if meta.len() > max_bytes as u64 {
                return Err(format!("the file is larger than {max_bytes} bytes"));
            }
            if let Ok(bytes) = std::fs::read(path) {
                return Ok(bytes);
            }
        }
    }
    let vm = unsafe { jni::JavaVM::from_raw(handles.vm.cast()) }.map_err(err)?;
    let mut env = vm.attach_current_thread().map_err(err)?;
    let activity = unsafe { JObject::from_raw(handles.activity.cast()) };

    let resolver = env
        .call_method(
            &activity,
            "getContentResolver",
            "()Landroid/content/ContentResolver;",
            &[],
        )
        .and_then(|v| v.l())
        .map_err(err)?;

    let uri_string = env.new_string(uri).map_err(err)?;
    let parsed = env
        .call_static_method(
            "android/net/Uri",
            "parse",
            "(Ljava/lang/String;)Landroid/net/Uri;",
            &[JValue::Object(&uri_string)],
        )
        .and_then(|v| v.l())
        .map_err(err)?;

    let stream = env
        .call_method(
            &resolver,
            "openInputStream",
            "(Landroid/net/Uri;)Ljava/io/InputStream;",
            &[JValue::Object(&parsed)],
        )
        .and_then(|v| v.l())
        .map_err(|e| {
            let _ = env.exception_clear();
            format!("the file behind the link could not be opened: {e}")
        })?;
    if stream.is_null() {
        return Err("the system returned no stream for that file".to_string());
    }

    let buffer: JByteArray = env.new_byte_array(64 * 1024).map_err(err)?;
    let mut bytes = Vec::new();
    loop {
        let read = env
            .call_method(&stream, "read", "([B)I", &[JValue::Object(&buffer)])
            .and_then(|v| v.i())
            .map_err(|e| {
                let _ = env.exception_clear();
                format!("reading the file failed partway: {e}")
            })?;
        if read <= 0 {
            break;
        }
        let mut chunk = vec![0i8; read as usize];
        env.get_byte_array_region(&buffer, 0, &mut chunk)
            .map_err(err)?;
        bytes.extend(chunk.into_iter().map(|b| b as u8));
        if bytes.len() > max_bytes {
            let _ = env.call_method(&stream, "close", "()V", &[]);
            return Err("that file is larger than any Krate app".to_string());
        }
    }
    let _ = env.call_method(&stream, "close", "()V", &[]);
    Ok(bytes)
}

fn get_string(env: &mut jni::JNIEnv, obj: JObject) -> Option<String> {
    if obj.is_null() {
        return None;
    }
    let jstring = JString::from(obj);
    env.get_string(&jstring).ok().map(|s| s.into())
}

/// The activity's cache folder, the app's own temporary space.
pub fn cache_dir(handles: ActivityHandles) -> Option<std::path::PathBuf> {
    let found = (|| -> Result<Option<String>, String> {
        let vm = unsafe { jni::JavaVM::from_raw(handles.vm.cast()) }.map_err(err)?;
        let mut env = vm.attach_current_thread().map_err(err)?;
        let activity = unsafe { JObject::from_raw(handles.activity.cast()) };
        let dir = env
            .call_method(&activity, "getCacheDir", "()Ljava/io/File;", &[])
            .and_then(|v| v.l())
            .map_err(err)?;
        if dir.is_null() {
            return Ok(None);
        }
        let path = env
            .call_method(&dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
            .and_then(|v| v.l())
            .map_err(err)?;
        Ok(get_string(&mut env, path))
    })();
    match found {
        Ok(path) => path.map(std::path::PathBuf::from),
        Err(why) => {
            log::warn!("no cache folder: {why}");
            None
        }
    }
}

/// Finish the activity, so Android does not bring it back when the process
/// ends.
pub fn finish(handles: ActivityHandles) {
    let finished = (|| -> Result<(), String> {
        let vm = unsafe { jni::JavaVM::from_raw(handles.vm.cast()) }.map_err(err)?;
        let mut env = vm.attach_current_thread().map_err(err)?;
        let activity = unsafe { JObject::from_raw(handles.activity.cast()) };
        env.call_method(&activity, "finish", "()V", &[])
            .map_err(err)?;
        Ok(())
    })();
    if let Err(why) = finished {
        log::warn!("could not finish the activity: {why}");
    }
}

fn err(e: impl core::fmt::Display) -> String {
    e.to_string()
}
