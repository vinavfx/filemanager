mod app;
mod instance;
mod media;
mod playback;
mod renderer;
mod resident;

use std::path::PathBuf;
use std::process::ExitCode;

fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if first
        .as_deref()
        .is_some_and(|arg| arg == "--help" || arg == "-h")
    {
        println!(
            "Usage: player [FILE | SEQUENCE_PATTERN]\n       player --check-media FILE\n       player --quit\n\nClose to keep Player in the background. Quit completely: Ctrl+Q or --quit."
        );
        return Ok(());
    }
    if first.as_deref().is_some_and(|arg| arg == "--check-media") {
        let path = args.next().ok_or("Missing media path")?;
        if args.next().is_some() {
            return Err("Expected one media path".into());
        }
        initialize_decoder_threads()?;
        return media::check_media(&PathBuf::from(path));
    }
    if args.next().is_some() {
        return Err("Expected one media path; use --help for usage".into());
    }
    let request = if first.as_deref().is_some_and(|arg| arg == "--quit") {
        instance::Request::Quit
    } else {
        instance::Request::Open(
            first
                .map(PathBuf::from)
                .map(std::path::absolute)
                .transpose()
                .map_err(|error| error.to_string())?,
        )
    };
    let Some(instance) = instance::Instance::start(&request).map_err(|error| error.to_string())?
    else {
        return Ok(());
    };
    let instance::Request::Open(path) = request else {
        return Ok(());
    };
    initialize_decoder_threads()?;
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Player")
            .with_inner_size([960.0, 600.0])
            .with_min_inner_size([640.0, 240.0]),
        renderer: eframe::Renderer::Glow,
        vsync: false,
        ..Default::default()
    };
    eframe::run_native(
        "Player",
        options,
        Box::new(move |context| {
            let requests = instance.listen(context.egui_ctx.clone())?;
            let player = app::Player::new(context, path)?;
            Ok(Box::new(resident::ResidentPlayer::new(
                player, instance, requests,
            )))
        }),
    )
    .map_err(|error| error.to_string())
}

fn initialize_decoder_threads() -> Result<(), String> {
    rayon_core::ThreadPoolBuilder::new()
        .num_threads(std::thread::available_parallelism().map_or(1, usize::from))
        .build_global()
        .map_err(|error| format!("Cannot initialize image decoder threads: {error}"))
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Player: {error}");
            ExitCode::FAILURE
        }
    }
}
