// Screen Switcher (Linux/COSMIC) - bootstrap del applet.
// Port del Screen Switcher de Windows (PowerShell + WPF) a un applet nativo
// del panel de COSMIC (Rust + libcosmic).

mod app;
mod brightness;
mod display;
mod profiles;
mod view;

fn main() -> cosmic::iced::Result {
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-V" | "--version" => {
                println!("cosmic-applet-screen-switcher {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            // Verificacion no destructiva: carga perfiles, lee el estado real
            // y detecta el perfil activo. No abre UI ni cambia nada.
            "--check" => return check(),
            _ => {}
        }
    }
    let cfg = profiles::load();
    cosmic::applet::run::<app::AppState>(cfg)
}

fn check() -> cosmic::iced::Result {
    let cfg = profiles::load();
    println!("Perfiles cargados: {}", cfg.profiles.len());
    for p in &cfg.profiles {
        println!("  - {} ({})", p.name, p.label);
    }
    match display::read_state() {
        Ok(state) => {
            println!("\nSalidas (cosmic-randr):");
            for o in &state {
                println!(
                    "  {} enabled={} pos={},{} transform={} modo={}x{}@{:.3}Hz primary={} [{} {}]",
                    o.name,
                    o.enabled,
                    o.x,
                    o.y,
                    o.transform,
                    o.width,
                    o.height,
                    o.refresh_mhz as f32 / 1000.0,
                    o.primary,
                    o.make,
                    o.model
                );
            }
            match display::detect_active(&cfg.profiles, &state) {
                Some(n) => println!("\nPerfil activo detectado: {n}"),
                None => println!("\nNingun perfil coincide con el estado actual."),
            }
        }
        Err(e) => {
            eprintln!("Error leyendo estado: {e}");
            std::process::exit(1);
        }
    }
    Ok(())
}
