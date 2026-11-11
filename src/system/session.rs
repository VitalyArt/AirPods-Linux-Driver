//! A graphical daemon must not outlive the login session that owns its UI.
use dbus::blocking::{Connection, stdintf::org_freedesktop_dbus::Properties};
use std::time::Duration;

const LOGIN: &str = "org.freedesktop.login1";
const SESSION: &str = "org.freedesktop.login1.Session";

pub fn start() {
    std::thread::spawn(|| {
        if let Err(error) = watch() {
            // Some desktops/sandboxes do not expose logind. Failure to monitor
            // must not prevent the application from running there.
            log::warn!("Cannot monitor the graphical login session: {error}");
        }
    });
}

fn watch() -> Result<(), dbus::Error> {
    let connection = Connection::new_system()?;
    let manager = connection.with_proxy(LOGIN, "/org/freedesktop/login1", Duration::from_secs(3));
    // Desktop launchers may run in the user manager rather than the session
    // scope. Prefer the inherited session ID, then try the process's session.
    let (path,): (dbus::Path<'static>,) = match std::env::var("XDG_SESSION_ID") {
        Ok(id) if !id.is_empty() => manager.method_call(LOGIN, "GetSession", (id,))?,
        _ => manager.method_call(LOGIN, "GetSessionByPID", (std::process::id(),))?,
    };
    let session = connection.with_proxy(LOGIN, path, Duration::from_secs(3));
    loop {
        if ended(session.get::<String>(SESSION, "State"))? {
            log::info!("Login session ended; exiting Airpods Driver.");
            // Also stop Bluetooth/tray worker threads and release the instance
            // lock, even when iced has no open windows to receive logout events.
            std::process::exit(0);
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn ended(state: Result<String, dbus::Error>) -> Result<bool, dbus::Error> {
    match state {
        Ok(state) => Ok(state == "closing"),
        Err(error)
            if matches!(
                error.name(),
                Some(
                    "org.freedesktop.DBus.Error.UnknownObject"
                        | "org.freedesktop.login1.NoSuchSession"
                )
            ) =>
        {
            Ok(true)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exits_on_logout_but_not_on_lock_or_user_switch() {
        for state in ["active", "online", "opening"] {
            assert!(!ended(Ok(state.into())).unwrap());
        }
        assert!(ended(Ok("closing".into())).unwrap());
    }

    #[test]
    fn removed_session_ends_but_bus_failure_does_not() {
        for name in [
            "org.freedesktop.DBus.Error.UnknownObject",
            "org.freedesktop.login1.NoSuchSession",
        ] {
            assert!(ended(Err(dbus::Error::new_custom(name, "gone"))).unwrap());
        }
        assert!(
            ended(Err(dbus::Error::new_custom(
                "org.freedesktop.DBus.Error.Disconnected",
                "bus restarted"
            )))
            .is_err()
        );
    }
}
