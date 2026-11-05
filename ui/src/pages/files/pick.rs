//! Files from the file picker and from a drop, with their folder structure: a picked folder keeps its
//! `webkitRelativePath`, and a dropped folder is walked with the browser's entries API so `photos/2026/a.jpg`
//! lands as `photos/2026/a.jpg`.

use super::http::promise;
use super::upload::Picked;
use js_sys::{Array, Function, Promise, Reflect};
use wasm_bindgen::{JsCast, JsValue};

fn get(o: &JsValue, k: &str) -> JsValue {
    Reflect::get(o, &JsValue::from_str(k)).unwrap_or(JsValue::UNDEFINED)
}

/// The files of a file input, each with its path inside a picked folder (or just its name).
pub fn from_input(input: &web_sys::HtmlInputElement) -> Vec<Picked> {
    let Some(list) = input.files() else { return Vec::new() };
    (0..list.length())
        .filter_map(|i| list.get(i))
        .map(|file| {
            let rel = get(file.as_ref(), "webkitRelativePath").as_string().filter(|s| !s.is_empty()).unwrap_or_else(|| file.name());
            Picked { file, relative: rel }
        })
        .collect()
}

/// Calls `entry.<method>(resolve, reject)` and waits for it.
async fn call_cb(target: &JsValue, method: &str) -> Result<JsValue, JsValue> {
    let f: Function = get(target, method).dyn_into().map_err(|_| JsValue::from_str("not supported"))?;
    let t = target.clone();
    let p = Promise::new(&mut |resolve, reject| {
        let _ = f.call2(&t, &resolve, &reject);
    });
    promise(p).await
}

/// The files of a drop, walking into dropped folders (the flat list when the browser has no entries API).
pub async fn from_drop(data_transfer: &JsValue) -> Vec<Picked> {
    let items = get(data_transfer, "items");
    let n = get(&items, "length").as_f64().unwrap_or(0.0) as u32;
    let mut entries = Vec::new();
    for i in 0..n {
        let item = Reflect::get_u32(&items, i).unwrap_or(JsValue::UNDEFINED);
        if get(&item, "kind").as_string().as_deref() != Some("file") {
            continue;
        }
        if let Ok(f) = get(&item, "webkitGetAsEntry").dyn_into::<Function>() {
            if let Ok(e) = f.call0(&item) {
                if !e.is_null() && !e.is_undefined() {
                    entries.push(e);
                }
            }
        }
    }
    if entries.is_empty() {
        let files = get(data_transfer, "files");
        let n = get(&files, "length").as_f64().unwrap_or(0.0) as u32;
        return (0..n)
            .filter_map(|i| Reflect::get_u32(&files, i).ok()?.dyn_into::<web_sys::File>().ok())
            .map(|file| Picked { relative: file.name(), file })
            .collect();
    }
    let mut out = Vec::new();
    // walk without recursion: (entry, prefix)
    let mut stack: Vec<(JsValue, String)> = entries.into_iter().rev().map(|e| (e, String::new())).collect();
    while let Some((entry, prefix)) = stack.pop() {
        let name = get(&entry, "name").as_string().unwrap_or_default();
        if get(&entry, "isFile").as_bool() == Some(true) {
            if let Ok(f) = call_cb(&entry, "file").await {
                if let Ok(file) = f.dyn_into::<web_sys::File>() {
                    out.push(Picked { file, relative: format!("{prefix}{name}") });
                }
            }
        } else if get(&entry, "isDirectory").as_bool() == Some(true) {
            let Ok(reader) = get(&entry, "createReader").dyn_into::<Function>().and_then(|f| f.call0(&entry)) else {
                continue;
            };
            let sub = format!("{prefix}{name}/");
            let mut children = Vec::new();
            // readEntries answers in batches until an empty one
            loop {
                let Ok(batch) = call_cb(&reader, "readEntries").await else { break };
                let batch = Array::from(&batch);
                if batch.length() == 0 {
                    break;
                }
                children.extend(batch.iter());
            }
            for c in children.into_iter().rev() {
                stack.push((c, sub.clone()));
            }
        }
    }
    out
}
