use std::fmt;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use sysinfo::{Cpu, Process, System};

// ─────────────────────────────────────────────
// ÉTAPE 1 — Modélisation des données
// ─────────────────────────────────────────────

#[derive(Debug, Clone)]
struct CpuInfo {
    usage_percent: f32,
    core_count: usize,
}

impl fmt::Display for CpuInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let filled = (self.usage_percent / 5.0).round() as usize; // barre sur 20 chars
        let bar: String = format!(
            "[{}{}]",
            "█".repeat(filled),
            "░".repeat(20usize.saturating_sub(filled))
        );
        write!(
            f,
            "CPU  {} {:5.1}%  ({} cœurs)",
            bar, self.usage_percent, self.core_count
        )
    }
}

#[derive(Debug, Clone)]
struct MemInfo {
    total_mb: u64,
    used_mb: u64,
}

impl fmt::Display for MemInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pct = self.used_mb as f32 / self.total_mb as f32 * 100.0;
        let filled = (pct / 5.0).round() as usize;
        let bar: String = format!(
            "[{}{}]",
            "█".repeat(filled),
            "░".repeat(20usize.saturating_sub(filled))
        );
        write!(
            f,
            "RAM  {} {:5.1}%  ({}/{} Mo)",
            bar, pct, self.used_mb, self.total_mb
        )
    }
}

#[derive(Debug, Clone)]
struct ProcessInfo {
    pid: u32,
    name: String,
    cpu_percent: f32,
    mem_mb: u64,
}

impl fmt::Display for ProcessInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "  {:6}  {:20}  {:5.1}% CPU  {:6} Mo",
            self.pid, self.name, self.cpu_percent, self.mem_mb
        )
    }
}

#[derive(Debug, Clone)]
struct SystemSnapshot {
    cpu: CpuInfo,
    mem: MemInfo,
    processes: Vec<ProcessInfo>,
    timestamp: String,
}

impl fmt::Display for SystemSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "┌─────────────────────────────────────────────┐")?;
        writeln!(f, "│           SysWatch — {}           │", self.timestamp)?;
        writeln!(f, "├─────────────────────────────────────────────┤")?;
        writeln!(f, "│ {}  │", self.cpu)?;
        writeln!(f, "│ {}  │", self.mem)?;
        writeln!(f, "├─────────────────────────────────────────────┤")?;
        writeln!(f, "│  PID     NOM                   CPU      RAM  │")?;
        for p in &self.processes {
            writeln!(f, "│{}│", p)?;
        }
        write!(f, "└─────────────────────────────────────────────┘")
    }
}

// ─────────────────────────────────────────────
// ÉTAPE 2 — Collecte réelle & gestion d'erreurs
// ─────────────────────────────────────────────

#[derive(Debug)]
enum SysWatchError {
    CollectError(String),
}

impl fmt::Display for SysWatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SysWatchError::CollectError(msg) => write!(f, "Erreur de collecte : {}", msg),
        }
    }
}

fn collect_snapshot() -> Result<SystemSnapshot, SysWatchError> {
    let mut sys = System::new_all();
    sys.refresh_all();

    // CPU global (moyenne sur tous les cœurs)
    let usage_percent = sys.global_cpu_info().cpu_usage();
    let core_count = sys.cpus().len();
    let cpu = CpuInfo { usage_percent, core_count };

    // RAM
    let total_mb = sys.total_memory() / 1024 / 1024;
    let used_mb  = sys.used_memory()  / 1024 / 1024;
    let mem = MemInfo { total_mb, used_mb };

    // Processus — top 5 CPU
    let mut processes: Vec<ProcessInfo> = sys
        .processes()
        .values()
        .map(|p| ProcessInfo {
            pid: p.pid().as_u32(),
            name: p.name().to_string(),
            cpu_percent: p.cpu_usage(),
            mem_mb: p.memory() / 1024 / 1024,
        })
        .collect();

    processes.sort_by(|a, b| b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap());
    processes.truncate(5);

    // Horodatage simple (sans dépendance chrono)
    let timestamp = "en cours".to_string(); // remplacé à l'étape suivante si besoin

    Ok(SystemSnapshot { cpu, mem, processes, timestamp })
}

// ─────────────────────────────────────────────
// ÉTAPE 3 — Formatage des réponses réseau
// ─────────────────────────────────────────────

fn format_response(snapshot: &SystemSnapshot, command: &str) -> String {
    let cmd = command.trim().to_lowercase();

    match cmd.as_str() {
        // ── cpu ──────────────────────────────────────────────────────────
        "cpu" => {
            let filled = (snapshot.cpu.usage_percent / 5.0).round() as usize;
            let bar = format!(
                "[{}{}]",
                "█".repeat(filled),
                "░".repeat(20usize.saturating_sub(filled))
            );
            format!(
                "=== CPU ===\n{} {:5.1}%\nCœurs : {}\n",
                bar, snapshot.cpu.usage_percent, snapshot.cpu.core_count
            )
        }

        // ── mem ──────────────────────────────────────────────────────────
        "mem" => {
            let pct =
                snapshot.mem.used_mb as f32 / snapshot.mem.total_mb as f32 * 100.0;
            let filled = (pct / 5.0).round() as usize;
            let bar = format!(
                "[{}{}]",
                "█".repeat(filled),
                "░".repeat(20usize.saturating_sub(filled))
            );
            format!(
                "=== MÉMOIRE ===\n{} {:5.1}%\nUtilisée : {} Mo / {} Mo\nLibre    : {} Mo\n",
                bar,
                pct,
                snapshot.mem.used_mb,
                snapshot.mem.total_mb,
                snapshot.mem.total_mb - snapshot.mem.used_mb
            )
        }

        // ── ps ───────────────────────────────────────────────────────────
        "ps" => {
            let header =
                "=== PROCESSUS (top 5 CPU) ===\n  PID     NOM                   CPU      RAM\n";
            let rows: String = snapshot
                .processes
                .iter()
                .map(|p| format!("{}\n", p))
                .collect();
            format!("{}{}", header, rows)
        }

        // ── all ──────────────────────────────────────────────────────────
        "all" => {
            // Réutilise les trois blocs ci-dessus
            let cpu_block  = format_response(snapshot, "cpu");
            let mem_block  = format_response(snapshot, "mem");
            let ps_block   = format_response(snapshot, "ps");
            format!("{}\n{}\n{}", cpu_block, mem_block, ps_block)
        }

        // ── help ─────────────────────────────────────────────────────────
        "help" => {
            "\
=== AIDE SysWatch ===
  cpu   — usage CPU global + barre ASCII
  mem   — mémoire utilisée / disponible
  ps    — top 5 processus par CPU
  all   — tout afficher
  help  — cette aide
  quit  — fermer la connexion
"
            .to_string()
        }

        // ── quit ─────────────────────────────────────────────────────────
        "quit" => "Au revoir !\n".to_string(),

        // ── commande inconnue ─────────────────────────────────────────────
        other => format!(
            "Commande inconnue : '{}'. Tape 'help' pour la liste.\n",
            other
        ),
    }
}

// ─────────────────────────────────────────────
// ÉTAPE 5 — Journalisation fichier
// ─────────────────────────────────────────────

/// Écrit une ligne horodatée dans syswatch.log (mode append).
fn log_event(message: &str) {
    // Horodatage en secondes depuis UNIX_EPOCH (pas besoin de chrono)
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Conversion manuelle en hh:mm:ss pour l'affichage
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    let line = format!("[{:02}:{:02}:{:02}] {}\n", h, m, s, message);

    // OpenOptions::append → crée le fichier s'il n'existe pas, sinon ajoute à la fin
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open("syswatch.log")
    {
        let _ = file.write_all(line.as_bytes());
    }
}

// ─────────────────────────────────────────────
// ÉTAPE 4 — Serveur TCP multi-threadé
// ─────────────────────────────────────────────

fn handle_client(stream: TcpStream, shared: Arc<Mutex<SystemSnapshot>>) {
    let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or("?".into());
    println!("[+] Connexion : {}", peer);
    log_event(&format!("CONNEXION {}", peer));  // ← étape 5

    let reader = BufReader::new(stream.try_clone().expect("clone stream"));
    let mut writer = stream;

    let _ = writer.write_all(b"Bienvenue sur SysWatch ! Tape 'help' pour les commandes.\n> ");

    for line in reader.lines() {
        let cmd = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        let cmd = cmd.trim().to_lowercase();
        println!("[{}] commande : '{}'", peer, cmd);
        log_event(&format!("COMMANDE {} > {}", peer, cmd));  // ← étape 5

        let snapshot = {
            let lock = shared.lock().unwrap();
            lock.clone()
        };

        let response = format_response(&snapshot, &cmd);
        let _ = writer.write_all(response.as_bytes());

        if cmd == "quit" {
            break;
        }

        let _ = writer.write_all(b"> ");
    }

    println!("[-] Déconnexion : {}", peer);
    log_event(&format!("DECONNEXION {}", peer));  // ← étape 5
}

fn main() {
    // ── 1. Premier snapshot pour initialiser le cache ──────────────────
    let initial = collect_snapshot().unwrap_or_else(|e| {
        eprintln!("Impossible de collecter les métriques : {}", e);
        std::process::exit(1);
    });

    // ── 2. Partage du snapshot via Arc<Mutex<>> ─────────────────────────
    //
    //  Arc  = Atomic Reference Count  → permet de partager la valeur
    //          entre plusieurs threads sans la copier.
    //  Mutex = Mutual Exclusion       → garantit qu'un seul thread
    //          lit/écrit à la fois (évite les data races).
    //
    let shared: Arc<Mutex<SystemSnapshot>> = Arc::new(Mutex::new(initial));

    // ── 3. Thread de rafraîchissement toutes les 5 secondes ────────────
    let shared_refresh = Arc::clone(&shared); // clone du pointeur (pas des données)
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(5));
        match collect_snapshot() {
            Ok(snap) => {
                let mut lock = shared_refresh.lock().unwrap();
                *lock = snap; // remplacement atomique du snapshot
                println!("[refresh] métriques mises à jour");
            }
            Err(e) => eprintln!("[refresh] erreur : {}", e),
        }
    });

    // ── 4. Écoute TCP sur le port 7878 ──────────────────────────────────
    let listener = TcpListener::bind("0.0.0.0:7878").expect("Impossible de binder le port 7878");
    println!("SysWatch écoute sur le port 7878 — connecte-toi avec : nc localhost 7878");
    log_event("SERVEUR DÉMARRÉ sur le port 7878");  // ← étape 5

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                // Clone du pointeur Arc pour le nouveau thread client
                let shared_client = Arc::clone(&shared);
                thread::spawn(move || handle_client(stream, shared_client));
            }
            Err(e) => eprintln!("Connexion refusée : {}", e),
        }
    }
}
