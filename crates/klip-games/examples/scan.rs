//! Prints what the library scan finds, the protection state of every game and the program
//! in the foreground. Read only: opens no window, starts nothing, sends no input.
//!
//!   cargo run -p klip-games --example scan
//!   cargo run -p klip-games --example scan --release -- --watch 120
//!   cargo run -p klip-games --example scan --release -- --watch 120 --track-foreground
//!
//! `--watch <seconds>` runs the watcher and measures what it costs. `--track-foreground`
//! adds the current foreground program as a manual game first, so the measurement covers
//! the case "a game is running" without a game having to be started. `--empty-catalog`
//! measures the opposite case, a watcher that knows no game at all.

use klip_games::foreground::{process_list, query_process_path, ProcessQuery};
use klip_games::{foreground_info, GameCatalog, GameWatcher, Identity, ManualGame, ManualList, WatcherOptions};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

/// CPU time this process has used so far (kernel plus user), from Windows' own accounting.
fn own_cpu_time() -> Duration {
    let (mut created, mut exited, mut kernel, mut user) =
        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe { GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user) }
        .expect("GetProcessTimes on the own process");
    let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
    Duration::from_nanos((ticks(kernel) + ticks(user)) * 100)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let watch = args.iter().position(|a| a == "--watch").and_then(|i| args.get(i + 1)).and_then(|s| s.parse::<u64>().ok());
    let track_foreground = args.iter().any(|a| a == "--track-foreground");

    let mut manual = ManualList::default();
    if track_foreground {
        if let Some(info) = foreground_info() {
            if let Some(name) = info.exe_name.clone() {
                println!("Tracking the foreground program as a manual game: {name}");
                manual.add(ManualGame { name: format!("{name} (test)"), exe_name: name, exe_path: info.exe_path.clone() });
            }
        }
    }

    // The four scans one by one instead of `GameCatalog::scan`, to show what each costs.
    let started = Instant::now();
    let mut problems = Vec::new();
    let mut games = Vec::new();
    type Scan = fn(&mut Vec<String>) -> Vec<klip_games::InstalledGame>;
    let scans: [(&str, Scan); 4] = [
        ("Steam", klip_games::library::scan_steam),
        ("Epic", klip_games::library::scan_epic),
        ("Riot", klip_games::library::scan_riot),
        ("Xbox", klip_games::library::scan_xbox),
    ];
    for (label, scan) in scans {
        let t = Instant::now();
        let found = scan(&mut problems);
        println!("{label}: {} games in {:.0} ms", found.len(), t.elapsed().as_secs_f64() * 1000.0);
        games.extend(found);
    }
    games.sort_by_key(|g| g.name.to_lowercase());
    if args.iter().any(|a| a == "--empty-catalog") {
        // Measures the watcher as if no game were installed: whatever is in the foreground
        // stays unknown, so every tick takes the cheapest path.
        println!("Dropping all {} games from the catalog for the measurement", games.len());
        games.clear();
    }
    let catalog = GameCatalog::new(games, manual, klip_games::ProtectionDb::probe());
    let scan_time = started.elapsed();

    let db = catalog.protection_db();
    println!("Machine: Vanguard installed = {}, FACEIT AC installed = {}", db.vanguard_installed, db.faceit_installed);
    println!("Library scan: {} games in {:.0} ms\n", catalog.games().len(), scan_time.as_secs_f64() * 1000.0);

    for game in catalog.games() {
        let protection = catalog.protection_of(game);
        println!("[{}] {}  ({})", game.source.label(), game.name, game.id);
        println!("    folder: {}", game.install_dir.display());
        println!("    exes:   {}", if game.exe_names.is_empty() { "none found".into() } else { game.exe_names.join(", ") });
        println!("    state:  {protection}  | hook allowed: {}", protection.hook_allowed());
    }
    if !problems.is_empty() {
        println!("\nProblems:");
        for problem in &problems {
            println!("    {problem}");
        }
    }

    println!("\nRunning games (process list, path with the limited query right only):");
    let mut any = false;
    // Only processes whose name belongs to a known game are asked for their path.
    for process in process_list().into_iter().filter(|p| catalog.knows_exe_name(&p.exe_name)) {
        let path = match query_process_path(process.pid) {
            ProcessQuery::Path(path) => Some(path),
            ProcessQuery::Denied | ProcessQuery::Gone => None,
        };
        if let Identity::Game(game) = catalog.identify(path.as_deref(), &process.exe_name) {
            any = true;
            let how = if path.is_some() { "path" } else { "name only, path refused" };
            println!("    pid {:>6}  {}  -> {} [{}]  {}  ({how})", process.pid, process.exe_name, game.name, game.source.label(), game.protection);
        }
    }
    if !any {
        println!("    none");
    }

    println!("\nForeground:");
    match foreground_info() {
        None => println!("    no foreground window"),
        Some(info) => {
            let name = info.exe_name.clone().unwrap_or_default();
            println!("    pid {}  {}", info.pid, info.exe_path.as_ref().map(|p| p.display().to_string()).unwrap_or(format!("{name} (path refused)")));
            println!("    covers its monitor: {}", info.covers_monitor);
            match catalog.identify(info.exe_path.as_deref(), &name) {
                Identity::Game(game) => println!("    game: {} [{}], {}", game.name, game.source.label(), game.protection),
                Identity::Excluded => println!("    not a game (excluded)"),
                Identity::Unknown(protection) => println!("    unknown program, {protection}"),
            }
        }
    }

    if let Some(seconds) = watch {
        println!("\nWatching for {seconds} s (interval 2 s) ...");
        let cpu_before = own_cpu_time();
        let wall = Instant::now();
        let (watcher, events) = GameWatcher::start(catalog, WatcherOptions::default());
        let end = wall + Duration::from_secs(seconds);
        while let Some(left) = end.checked_duration_since(Instant::now()) {
            match events.recv_timeout(left) {
                Ok(event) => println!("    {:>6.1} s  {event:?}", wall.elapsed().as_secs_f64()),
                Err(_) => break,
            }
        }
        let stats = watcher.stats();
        let elapsed = wall.elapsed();
        let cpu = own_cpu_time() - cpu_before;
        watcher.stop();
        println!("Ticks: {}, of which resolved again: {}", stats.ticks, stats.resolves);
        println!(
            "Time spent working in the watcher: {:.3} ms in total, {:.1} microseconds per tick",
            stats.busy.as_secs_f64() * 1000.0,
            stats.busy.as_secs_f64() * 1e6 / stats.ticks.max(1) as f64
        );
        println!("Share of one core by that sum: {:.5} %", stats.busy.as_secs_f64() / elapsed.as_secs_f64() * 100.0);
        println!(
            "Process CPU time by GetProcessTimes over {:.1} s: {:.3} ms = {:.5} % of one core",
            elapsed.as_secs_f64(),
            cpu.as_secs_f64() * 1000.0,
            cpu.as_secs_f64() / elapsed.as_secs_f64() * 100.0
        );
    }
}
