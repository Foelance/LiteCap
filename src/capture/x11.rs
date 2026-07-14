use std::process::Command;

/// Monitor geometry parsed from `xrandr --listmonitors`, used to build the
/// x11grab `-video_size`/offset args. No in-process capture on X11: ffmpeg
/// reads the screen itself.
#[derive(Debug, Clone)]
pub struct MonGeom {
    pub name: String,
    pub w: u32,
    pub h: u32,
    pub x: i32,
    pub y: i32,
}

/// Parses lines of the form:
/// ` 0: +*eDP-1 1920/309x1080/174+0+0  eDP-1`
/// `xrandr` missing or unparsable -> empty Vec (caller records the whole
/// root screen).
pub fn monitors() -> Vec<MonGeom> {
    let Ok(output) = Command::new("xrandr").arg("--listmonitors").output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().skip(1).filter_map(parse_monitor_line).collect()
}

fn parse_monitor_line(line: &str) -> Option<MonGeom> {
    // Fields: "<idx>: <flags><WxH+X+Y as W/mmxH/mm+X+Y> <name>"
    let mut fields = line.split_whitespace();
    let _idx = fields.next()?;
    let geom_field = fields.next()?;
    let name = fields.next()?.to_string();

    let geom = geom_field.trim_start_matches(|c| c == '+' || c == '*');
    // geom looks like "1920/309x1080/174+0+0"
    let (wh, rest) = geom.split_once('x')?;
    let w: u32 = wh.split('/').next()?.parse().ok()?;
    let mut rest_parts = rest.splitn(3, '+');
    let h: u32 = rest_parts.next()?.split('/').next()?.parse().ok()?;
    let x: i32 = rest_parts.next()?.parse().ok()?;
    let y: i32 = rest_parts.next()?.parse().ok()?;

    Some(MonGeom { name, w, h, x, y })
}

/// True if the current session is Wayland (portal capture path required).
pub fn is_wayland_session() -> bool {
    std::env::var("XDG_SESSION_TYPE")
        .map(|v| v.eq_ignore_ascii_case("wayland"))
        .unwrap_or(false)
        || std::env::var("WAYLAND_DISPLAY").is_ok()
}
