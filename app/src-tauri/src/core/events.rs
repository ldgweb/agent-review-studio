// Push notifications from the core to the desktop window, so the frontend
// never polls. The desktop shell installs the emitter (Tauri's
// event system); the CLI installs none, so emitting is a no-op there.
//
//   ars://sessions-changed  a scan changed what the session index serves
//   ars://review-changed    the project under review changed on disk

use once_cell::sync::OnceCell;
use serde_json::Value;

type Emitter = Box<dyn Fn(&str, Value) + Send + Sync>;

static EMITTER: OnceCell<Emitter> = OnceCell::new();

pub fn set_emitter(f: impl Fn(&str, Value) + Send + Sync + 'static) {
    let _ = EMITTER.set(Box::new(f));
}

pub fn emit(name: &str, payload: Value) {
    if let Some(f) = EMITTER.get() {
        f(name, payload);
    }
}
