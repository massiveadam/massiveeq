//! Repeating editor callbacks must end with their window, even if GTK retains widgets.
use gtk4::glib::SourceId;
use std::cell::{Cell, RefCell};

#[derive(Default)]
pub struct EditorTasks {
    closed: Cell<bool>,
    sources: RefCell<Vec<SourceId>>,
}

impl EditorTasks {
    pub fn track(&self, source: SourceId) {
        if self.is_closed() {
            source.remove();
        } else {
            self.sources.borrow_mut().push(source);
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.get()
    }

    pub fn close(&self) {
        self.closed.set(true);
        for source in self.sources.take() {
            source.remove();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk4::glib::{self, ControlFlow, MainContext};
    use std::{rc::Rc, time::Duration};

    #[test]
    fn closing_releases_periodic_callbacks_and_rejects_new_timers() {
        let context = MainContext::default();
        let _guard = context.acquire().unwrap();
        let tasks = EditorTasks::default();
        let retained_editor = Rc::new(());
        let weak = Rc::downgrade(&retained_editor);
        tasks.track(glib::timeout_add_local(Duration::from_secs(2), move || {
            let _keep_alive = &retained_editor;
            ControlFlow::Continue
        }));
        assert!(weak.upgrade().is_some());
        tasks.close();
        assert!(
            weak.upgrade().is_none(),
            "closed callbacks retain the old editor"
        );
        tasks.close();
        let late = Rc::new(());
        let weak = Rc::downgrade(&late);
        tasks.track(glib::timeout_add_local(Duration::from_secs(2), move || {
            let _keep_alive = &late;
            ControlFlow::Continue
        }));
        assert!(weak.upgrade().is_none(), "closed editors can rearm a timer");
    }
}
