use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, Window};
use x11rb::wrapper::ConnectionExt as _;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn center(parent: Option<Window>, name: &str) {
    let Some(parent) = parent else {
        return;
    };
    if let Err(error) = center_dialog(parent, name) {
        eprintln!("Cannot center properties window: {error}");
    }
}

fn center_dialog(parent: Window, name: &str) -> Result<()> {
    let (connection, screen) = x11rb::connect(None)?;
    let root = connection.setup().roots[screen].root;
    let atom =
        |name: &[u8]| -> Result<u32> { Ok(connection.intern_atom(false, name)?.reply()?.atom) };
    let clients = atom(b"_NET_CLIENT_LIST")?;
    let title = atom(b"_NET_WM_NAME")?;
    let window_type = atom(b"_NET_WM_WINDOW_TYPE")?;
    let dialog_type = atom(b"_NET_WM_WINDOW_TYPE_DIALOG")?;
    let extents = atom(b"_NET_FRAME_EXTENTS")?;
    let moveresize = atom(b"_NET_MOVERESIZE_WINDOW")?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        let windows = connection
            .get_property(false, root, clients, AtomEnum::WINDOW, 0, u32::MAX)?
            .reply()?;
        for window in windows.value32().into_iter().flatten() {
            let matches = (|| -> Result<bool> {
                let class = connection
                    .get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256)?
                    .reply()?;
                if !class
                    .value
                    .split(|byte| *byte == 0)
                    .any(|part| part == b"Caja")
                {
                    return Ok(false);
                }
                let kind = connection
                    .get_property(false, window, window_type, AtomEnum::ATOM, 0, 32)?
                    .reply()?;
                if !kind
                    .value32()
                    .into_iter()
                    .flatten()
                    .any(|kind| kind == dialog_type)
                {
                    return Ok(false);
                }
                let title = connection
                    .get_property(false, window, title, AtomEnum::ANY, 0, 1024)?
                    .reply()?;
                Ok(String::from_utf8_lossy(&title.value).starts_with(name))
            })();
            if !matches!(matches, Ok(true)) {
                continue;
            }
            let frame = |window| -> Result<(i32, i32, i32, i32)> {
                let geometry = connection.get_geometry(window)?.reply()?;
                let position = connection
                    .translate_coordinates(window, root, 0, 0)?
                    .reply()?;
                let borders = connection
                    .get_property(false, window, extents, AtomEnum::CARDINAL, 0, 4)?
                    .reply()?;
                let borders: Vec<_> = borders.value32().into_iter().flatten().collect();
                let [left, right, top, bottom] = match borders.as_slice() {
                    [left, right, top, bottom] => {
                        [*left, *right, *top, *bottom].map(|value| value as i32)
                    }
                    _ => [0; 4],
                };
                Ok((
                    i32::from(position.dst_x) - left,
                    i32::from(position.dst_y) - top,
                    i32::from(geometry.width) + left + right,
                    i32::from(geometry.height) + top + bottom,
                ))
            };
            let (x, y, width, height) = frame(parent)?;
            let (_, _, dialog_width, dialog_height) = frame(window)?;
            connection.change_property32(
                x11rb::protocol::xproto::PropMode::REPLACE,
                window,
                AtomEnum::WM_TRANSIENT_FOR,
                AtomEnum::WINDOW,
                &[parent],
            )?;
            connection.send_event(
                false,
                root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                ClientMessageEvent::new(
                    32,
                    window,
                    moveresize,
                    [
                        1 | (1 << 8) | (1 << 9) | (2 << 12),
                        (x + (width - dialog_width) / 2) as u32,
                        (y + (height - dialog_height) / 2) as u32,
                        0,
                        0,
                    ],
                ),
            )?;
            connection.flush()?;
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}
