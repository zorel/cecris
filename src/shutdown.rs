use std::sync::atomic::{AtomicBool, Ordering};

static MAINTENANCE: AtomicBool = AtomicBool::new(false);

pub fn maintenance() -> bool {
    MAINTENANCE.load(Ordering::SeqCst)
}

pub fn install() {
    #[cfg(unix)]
    {
        let handler: extern "C" fn(i32) = handle;
        unsafe {
            signal(15, handler as usize);
        }
    }
}

#[cfg(unix)]
extern "C" fn handle(_signal: i32) {
    MAINTENANCE.store(true, Ordering::SeqCst);
}

#[cfg(unix)]
unsafe extern "C" {
    fn signal(signum: i32, handler: usize) -> usize;
}
