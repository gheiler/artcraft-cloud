//! The apps' own file pickers, answered from the drive.
//!
//! On the web the apps open files through a hidden `<input type=file>` (WordCraft's File ▸ Open,
//! Insert ▸ Picture, LightCraft's Restore Library from Backup…) or `showOpenFilePicker`. Inside
//! artcraftCloud the user's files are on the server, so once the app frame has loaded (same
//! origin) the shell wraps those two entry points of the frame: instead of the browser's dialog the
//! shell's own picker opens over the drive, with "From this device" as a fallback. The chosen
//! file goes back into the app's input exactly as if the browser had picked it.
//!
//! This is the only hand-written JavaScript in the shell: wrapping a method of the frame's
//! `HTMLInputElement.prototype` needs a function that receives `this`, which Rust closures cannot
//! be. See docs/upstream-gaps.md for the host message that would make it unnecessary.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = r#"
export function wrap_pickers(win, pick) {
  if (!win || win.__artcraftPickers) return;
  win.__artcraftPickers = true;
  const proto = win.HTMLInputElement.prototype;
  const click = proto.click;
  proto.click = function () {
    if (this.type === 'file') { pick(this, this.accept || '', !!this.multiple); return; }
    return click.apply(this, arguments);
  };
  if (proto.showPicker) {
    const show = proto.showPicker;
    proto.showPicker = function () {
      if (this.type === 'file') { pick(this, this.accept || '', !!this.multiple); return; }
      return show.apply(this, arguments);
    };
  }
  if (win.showOpenFilePicker) {
    win.showOpenFilePicker = function (options) {
      return new Promise(function (resolve, reject) {
        const types = (options && options.types) || [];
        const accept = types.map(function (t) { return Object.values(t.accept || {}).flat().join(','); }).join(',');
        pick({ resolve: resolve, reject: reject, win: win }, accept, !!(options && options.multiple));
      });
    };
  }
}
export function deliver(target, names, buffers, types) {
  const make = function (win) {
    return names.map(function (name, i) { return new win.File([buffers[i]], name, { type: types[i] || '' }); });
  };
  if (target.resolve) {
    const files = make(target.win);
    target.resolve(files.map(function (file) { return { kind: 'file', name: file.name, getFile: function () { return Promise.resolve(file); } }; }));
    return;
  }
  const win = target.ownerDocument.defaultView;
  const transfer = new win.DataTransfer();
  make(win).forEach(function (file) { transfer.items.add(file); });
  target.files = transfer.files;
  target.dispatchEvent(new win.Event('input', { bubbles: true }));
  target.dispatchEvent(new win.Event('change', { bubbles: true }));
}
export function cancel(target) {
  if (target.reject) { target.reject(new target.win.DOMException('The user aborted a request.', 'AbortError')); return; }
  const win = target.ownerDocument && target.ownerDocument.defaultView;
  if (win) target.dispatchEvent(new win.Event('cancel'));
}
"#)]
extern "C" {
    fn wrap_pickers(win: &JsValue, pick: &js_sys::Function);
    fn deliver(
        target: &JsValue,
        names: js_sys::Array,
        buffers: js_sys::Array,
        types: js_sys::Array,
    );
    fn cancel(target: &JsValue);
}

thread_local! {
    static TARGETS: RefCell<HashMap<u32, JsValue>> = RefCell::new(HashMap::new());
    static NEXT: Cell<u32> = const { Cell::new(1) };
    static HANDLER: RefCell<Option<js_sys::Function>> = const { RefCell::new(None) };
}

/// Wrap the pickers of an app frame's window. `on_pick(id, accept, multiple)` opens the dialog.
pub fn install(window: &JsValue, on_pick: impl Fn(u32, String, bool) + 'static) {
    let handler = HANDLER.with(|slot| {
        if let Some(existing) = slot.borrow().as_ref() {
            return existing.clone();
        }
        let closure = Closure::<dyn Fn(JsValue, String, bool)>::new(
            move |target: JsValue, accept: String, multiple: bool| {
                let id = NEXT.with(|n| {
                    let id = n.get();
                    n.set(id.wrapping_add(1));
                    id
                });
                TARGETS.with(|targets| targets.borrow_mut().insert(id, target));
                on_pick(id, accept, multiple);
            },
        );
        let function: js_sys::Function = closure.into_js_value().unchecked_into();
        *slot.borrow_mut() = Some(function.clone());
        function
    });
    wrap_pickers(window, &handler);
}

/// Hand files to the app that asked (`id`), as if the browser had picked them.
pub fn answer(id: u32, files: Vec<(String, Vec<u8>)>) {
    let Some(target) = TARGETS.with(|targets| targets.borrow_mut().remove(&id)) else {
        return;
    };
    let names = js_sys::Array::new();
    let buffers = js_sys::Array::new();
    let types = js_sys::Array::new();
    for (name, bytes) in files {
        types.push(&JsValue::from_str(mime_of(&name)));
        names.push(&JsValue::from_str(&name));
        buffers.push(&js_sys::Uint8Array::from(bytes.as_slice()));
    }
    deliver(&target, names, buffers, types);
}

/// Tell the app its picker was dismissed.
pub fn dismiss(id: u32) {
    if let Some(target) = TARGETS.with(|targets| targets.borrow_mut().remove(&id)) {
        cancel(&target);
    }
}

/// Whether a file named `name` fits an `accept` list (`.docx,image/*,application/zip`).
pub fn accepts(accept: &str, name: &str) -> bool {
    let accept = accept.trim();
    if accept.is_empty() {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    let mime = mime_of(&lower);
    accept
        .split(',')
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .any(|item| {
            let item = item.to_ascii_lowercase();
            if item.starts_with('.') {
                lower.ends_with(&item)
            } else if let Some(prefix) = item.strip_suffix("/*") {
                mime.starts_with(&format!("{prefix}/"))
            } else {
                item == mime
            }
        })
}

pub fn mime_of(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "tif" | "tiff" => "image/tiff",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "txt" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        "rtf" => "application/rtf",
        "csv" => "text/csv",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::accepts;

    #[test]
    fn accept_lists_filter_names() {
        let word = ".docx,.docm,.doc,.odt,.rtf,.txt,.md";
        assert!(accepts(word, "Hola.docx"));
        assert!(accepts(word, "NOTES.TXT"));
        assert!(!accepts(word, "photo.jpg"));
        assert!(accepts("image/*", "photo.JPG"));
        assert!(accepts(
            ".zip,application/zip",
            "lightcraft-library-backup-2026-11-23.zip"
        ));
        assert!(!accepts(".zip,application/zip", "a.docx"));
        assert!(accepts("", "anything.bin"));
    }
}
