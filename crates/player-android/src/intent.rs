//! Reading the launching intent through JNI.
//!
//! The player is pure native code, so the intent -- which .krate file or
//! link the person tapped -- is fetched from the Java side by hand:
//! android-activity fills ndk-context with the JavaVM and the activity
//! object, and everything else is plain method calls. Failures never
//! panic; a player that cannot read its intent falls back to the demo,
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

/// The URI the activity was launched to VIEW, if any.
pub fn view_target(handles: ActivityHandles) -> Option<String> {
    match try_view_target(handles) {
        Ok(target) => target,
        Err(err) => {
            log::warn!("could not read the launch intent: {err}");
            None
        }
    }
}

fn try_view_target(handles: ActivityHandles) -> Result<Option<String>, String> {
    let vm = unsafe { jni::JavaVM::from_raw(handles.vm.cast()) }.map_err(err)?;
    let mut env = vm.attach_current_thread().map_err(err)?;
    let activity = unsafe { JObject::from_raw(handles.activity.cast()) };

    let intent = env
        .call_method(&activity, "getIntent", "()Landroid/content/Intent;", &[])
        .and_then(|v| v.l())
        .map_err(|e| {
            // The description goes to logcat as System.err before the
            // exception is cleared -- the only way to see which call blew.
            let _ = env.exception_describe();
            let _ = env.exception_clear();
            format!("getIntent: {e}")
        })?;
    if intent.is_null() {
        return Ok(None);
    }

    let action_obj = env
        .call_method(&intent, "getAction", "()Ljava/lang/String;", &[])
        .and_then(|v| v.l())
        .map_err(err)?;
    let action = get_string(&mut env, action_obj);
    if action.as_deref() != Some("android.intent.action.VIEW") {
        return Ok(None);
    }

    let data_obj = env
        .call_method(&intent, "getDataString", "()Ljava/lang/String;", &[])
        .and_then(|v| v.l())
        .map_err(err)?;
    Ok(get_string(&mut env, data_obj))
}

/// Read every byte behind a content:// or file:// URI through the
/// platform's ContentResolver -- the only path modern Android guarantees.
pub fn read_uri(handles: ActivityHandles, uri: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
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

fn err(e: impl core::fmt::Display) -> String {
    e.to_string()
}
