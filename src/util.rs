/// If contained Result is Err variant, log error
#[macro_export]
macro_rules! log_if_err {
    ($expression:expr) => {
        if let Err(e) = $expression {
            log::error!("{:?}", e);
        }
    };
}
