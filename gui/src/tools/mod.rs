//! Tool modules. Each tool lives in its own file under `tools/` and exposes
//! a `page(app)` and (optionally) a `settings_section(app)` entry point.
//!
//! State for each tool currently lives on `AppModel` (with `pub(crate)`
//! visibility for tool fns to read). When two tools coexist with non-trivial
//! state, this is the right place to introduce a `Tool` trait + per-tool
//! state structs; for one tool, free fns + shared state is simpler and just
//! as factored.

pub mod color_picker;
pub mod mouse_find;
