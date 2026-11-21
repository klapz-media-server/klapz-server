//! The notification-area icon on Windows: open the web client already signed in, or stop
//! the server.
//!
//! It runs its own event loop on a thread of its own, since a tray icon's window messages
//! arrive on the thread that made it. "Open web app" hands the browser a one-time sign-in
//! ticket in the address (see `signInWithTicket`); the client redeems it and takes it out of
//! the address, so the session is the usual cookie. "Quit" stops the server as Ctrl-C does.
//!
//! No desktop to show it on — the server running as a service — is no reason not to serve:
//! the icon is then left out with a warning. `KLAPZ_TRAY=0` leaves it out on purpose.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::platform::run_return::EventLoopExtRunReturn as _;
use tao::platform::windows::EventLoopBuilderExtWindows as _;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// What the icon's menu does, the server's side of it.
pub struct Actions {
    /// A new address of the web client carrying a sign-in ticket, or none when the server
    /// serves no client.
    pub web_app: Option<Box<dyn Fn() -> String + Send>>,
    /// Asks the server to stop.
    pub quit: Arc<tokio::sync::Notify>,
}

impl std::fmt::Debug for Actions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Actions")
            .field("web_app", &self.web_app.is_some())
            .finish_non_exhaustive()
    }
}

/// The running icon, removed when the server stops.
#[derive(Debug)]
pub struct Tray {
    proxy: EventLoopProxy<Message>,
    thread: std::thread::JoinHandle<()>,
}

#[derive(Debug)]
enum Message {
    Menu(MenuId),
    Close,
}

impl Tray {
    /// Shows the icon, unless `KLAPZ_TRAY` turns it off or there is no desktop to show it on.
    pub fn show(actions: Actions) -> Option<Self> {
        if std::env::var("KLAPZ_TRAY").is_ok_and(|value| matches!(value.as_str(), "0" | "off")) {
            return None;
        }
        let (sent, proxy) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("tray".to_owned())
            .spawn(move || run(actions, &sent))
            .ok()?;
        match proxy.recv() {
            Ok(proxy) => Some(Self { proxy, thread }),
            // The thread ended before its loop ran, having said why.
            Err(_) => None,
        }
    }

    /// Takes the icon away, rather than leaving it until the mouse passes over it.
    pub fn close(self) {
        if self.proxy.send_event(Message::Close).is_ok() {
            let _ = self.thread.join();
        }
    }
}

fn run(actions: Actions, sent: &std::sync::mpsc::Sender<EventLoopProxy<Message>>) {
    let mut event_loop = EventLoopBuilder::<Message>::with_user_event()
        .with_any_thread(true)
        .build();
    let proxy = event_loop.create_proxy();
    let forward = proxy.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let _ = forward.send_event(Message::Menu(event.id));
    }));

    let open = MenuItem::new("Open web app", actions.web_app.is_some(), None);
    let quit = MenuItem::new("Quit klapz", true, None);
    let menu = Menu::new();
    if let Err(error) = menu.append_items(&[&open, &PredefinedMenuItem::separator(), &quit]) {
        tracing::warn!(%error, "no tray icon: its menu cannot be made");
        return;
    }
    if sent.send(proxy).is_err() {
        return;
    }

    let mut icon: Option<TrayIcon> = None;
    event_loop.run_return(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(StartCause::Init) => {
                icon = make_icon(&menu);
            }
            Event::UserEvent(Message::Menu(id)) if id == *open.id() => {
                if let Some(address) = actions.web_app.as_ref().map(|make| make()) {
                    open_in_browser(&address);
                }
            }
            Event::UserEvent(Message::Menu(id)) if id == *quit.id() => {
                tracing::info!("stopping: asked from the tray icon");
                actions.quit.notify_one();
            }
            Event::UserEvent(Message::Close) => {
                icon.take();
                *flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}

fn make_icon(menu: &Menu) -> Option<TrayIcon> {
    let shown = Icon::from_rgba(icon_pixels(), ICON_SIZE, ICON_SIZE)
        .map_err(|error| error.to_string())
        .and_then(|icon| {
            TrayIconBuilder::new()
                .with_menu(Box::new(menu.clone()))
                .with_tooltip("klapz")
                .with_icon(icon)
                .build()
                .map_err(|error| error.to_string())
        });
    match shown {
        Ok(icon) => Some(icon),
        Err(error) => {
            tracing::warn!(%error, "no tray icon: there may be no desktop to show it on");
            None
        }
    }
}

/// Opens `address` in the default browser, as the shell opens a link.
fn open_in_browser(address: &str) {
    // `url.dll` hands the address to the default browser untouched: `cmd /c start` would
    // read `&` and `^` in it.
    let opened = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", address])
        .spawn();
    if let Err(error) = opened {
        tracing::warn!(%error, "could not open the browser");
    }
}

/// Where a browser on this machine reaches a server listening on `address`: a server
/// listening on every interface is reached on loopback.
pub fn local_origin(address: SocketAddr) -> String {
    let ip = match address.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    format!("http://{}", SocketAddr::new(ip, address.port()))
}

/// The icon's side, in pixels.
const SIDE: u8 = 32;
const ICON_SIZE: u32 = SIDE as u32;

/// The web client's favicon, drawn: a dark rounded square with an amber play mark.
fn icon_pixels() -> Vec<u8> {
    const RADIUS: f32 = 8.0;
    let side = f32::from(SIDE);
    let mut pixels = Vec::with_capacity(usize::from(SIDE) * usize::from(SIDE) * 4);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let (px, py) = (f32::from(x) + 0.5, f32::from(y) + 0.5);
            // How far outside the rounded corners' circles the pixel is.
            let dx = (RADIUS - px).max(px - (side - RADIUS)).max(0.0);
            let dy = (RADIUS - py).max(py - (side - RADIUS)).max(0.0);
            let inside = dx.hypot(dy) <= RADIUS;
            // The favicon's triangle, (11,8) (24,16) (11,24).
            let play = px >= 11.0 && (py - 16.0).abs() <= (24.0 - px) * 8.0 / 13.0;
            let pixel: [u8; 4] = match (inside, play) {
                (false, _) => [0, 0, 0, 0],
                (true, true) => [0xe5, 0xa0, 0x0d, 0xff],
                (true, false) => [0x1e, 0x1e, 0x1e, 0xff],
            };
            pixels.extend_from_slice(&pixel);
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_on_every_interface_is_opened_on_loopback() {
        let origin =
            |address: &str| local_origin(address.parse().unwrap_or_else(|_| unreachable!()));
        assert_eq!(origin("0.0.0.0:32500"), "http://127.0.0.1:32500");
        assert_eq!(origin("[::]:32500"), "http://[::1]:32500");
        assert_eq!(origin("192.168.1.4:32400"), "http://192.168.1.4:32400");
    }

    #[test]
    fn the_icon_is_a_square_with_its_corners_cut() {
        let pixels = icon_pixels();
        assert_eq!(pixels.len(), 32 * 32 * 4);
        let at = |x: usize, y: usize| &pixels[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4];
        assert_eq!(at(0, 0)[3], 0, "corner");
        assert_eq!(at(16, 2), [0x1e, 0x1e, 0x1e, 0xff], "background");
        assert_eq!(at(14, 16), [0xe5, 0xa0, 0x0d, 0xff], "play mark");
    }
}
