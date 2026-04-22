use std::fmt;
use std::thread;
use std::time::Duration;
use std::net::{TcpListener, TcpStream};
use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, Mutex};
use sysinfo::System;

// ============================================================
// ÉTAPE 1 — Modélisation des données
// ============================================================

#[derive(Debug, Clone)]
struct CpuInfo {
    usage_percent: f32,
    core_count: usize,
}

#[derive(Debug, Clone)]
struct MemInfo {
    total_mb: u64,
    used_mb: u64,
}

#[derive(Debug, Clone)]
struct ProcessInfo {
    pid: u32,
    name: String,
    cpu_percent: f32,
    mem_mb: u64,
}

#[derive(Debug, Clone)]
struct SystemSnapshot {
    cpu: CpuInfo,
    mem: MemInfo,
    processes: Vec<ProcessInfo>,
    timestamp: String,
}

impl fmt::Display for CpuInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CPU: {:.1}% ({} cœurs)", self.usage_percent, self.core_count)
    }
}

impl fmt::Display for MemInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RAM: {} Mo / {} Mo", self.used_mb, self.total_mb)
    }
}

impl fmt::Display for ProcessInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {} — CPU: {:.1}% MEM: {} Mo",
            self.pid, self.name, self.cpu_percent, self.mem_mb)
    }
}

impl fmt::Display for SystemSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "=== Snapshot [{}] ===", self.timestamp)?;
        writeln!(f, "{}", self.cpu)?;
        writeln!(f, "{}", self.mem)?;
        for p in &self.processes {
            writeln!(f, "  {}", p)?;
        }
        Ok(())
    }
}

// ============================================================
// ÉTAPE 2 — Collecte réelle et gestion d'erreurs
// ============================================================

#[derive(Debug)]
enum SysWatchError {
    CollectionError(String),
}

impl fmt::Display for SysWatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SysWatchError::CollectionError(msg) => write!(f, "Erreur de collecte : {}", msg),
        }
    }
}

fn collect_snapshot() -> Result<SystemSnapshot, SysWatchError> {
    let mut sys = System::new_all();

    sys.refresh_all();
    thread::sleep(Duration::from_millis(500));
    sys.refresh_all();

    let cpu_usage = sys.global_cpu_info().cpu_usage();
    let core_count = sys.cpus().len();

    let total_mb = sys.total_memory() / 1024 / 1024;
    let used_mb  = sys.used_memory()  / 1024 / 1024;

    let mut processes: Vec<ProcessInfo> = sys
        .processes()
        .iter()
        .map(|(pid, proc_)| ProcessInfo {
            pid: pid.as_u32(),
            name: proc_.name().to_string(),
            cpu_percent: proc_.cpu_usage(),
            mem_mb: proc_.memory() / 1024 / 1024,
        })
        .collect();

    processes.sort_by(|a, b| b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap());
    processes.truncate(5);

    let timestamp = "2025-01-01 12:00:00".to_string();

    Ok(SystemSnapshot {
        cpu: CpuInfo { usage_percent: cpu_usage, core_count },
        mem: MemInfo { total_mb, used_mb },
        processes,
        timestamp,
    })
}

// ============================================================
// ÉTAPE 3 — Formatage des réponses réseau
// ============================================================

fn format_response(snapshot: &SystemSnapshot, command: &str) -> String {
    match command.trim().to_lowercase().as_str() {
        "cpu" => format!("=== CPU ===\n{}\n", snapshot.cpu),

        "mem" => format!("=== MÉMOIRE ===\n{}\n", snapshot.mem),

        "ps" => {
            let mut out = String::from("=== PROCESSUS (Top 5 CPU) ===\n");
            for p in &snapshot.processes {
                out.push_str(&format!("  {}\n", p));
            }
            out
        }

        "all" => format!("{}", snapshot),

        "help" => String::from(
            "=== COMMANDES DISPONIBLES ===\n\
             cpu   → usage CPU\n\
             mem   → usage mémoire\n\
             ps    → top 5 processus\n\
             all   → snapshot complet\n\
             help  → ce message\n\
             quit  → quitter\n"
        ),

        "quit" => String::from("Au revoir !\n"),

        _ => format!("Commande inconnue : '{}'. Tapez 'help'.\n", command.trim()),
    }
}

// ============================================================
// ÉTAPE 4 — Gestion d'un client
// ============================================================

fn handle_client(stream: TcpStream, shared_snapshot: Arc<Mutex<SystemSnapshot>>) {
    let mut writer = stream.try_clone().expect("Impossible de cloner le stream");
    let reader = BufReader::new(stream);

    let banner = "Bienvenue sur SysWatch ! Tapez 'help' pour les commandes.\n> ";
    let _ = writer.write_all(banner.as_bytes());

    for line in reader.lines() {
        match line {
            Ok(cmd) => {
                let cmd = cmd.trim().to_string();
                if cmd.is_empty() {
                    let _ = writer.write_all(b"> ");
                    continue;
                }

                if cmd.to_lowercase() == "quit" {
                    let _ = writer.write_all(b"Au revoir !\n");
                    break;
                }

                let response = {
                    let snapshot = shared_snapshot.lock().unwrap();
                    format_response(&snapshot, &cmd)
                };

                let _ = writer.write_all(response.as_bytes());
                let _ = writer.write_all(b"> ");
            }
            Err(_) => break,
        }
    }
}

// ============================================================
// MAIN
// ============================================================

fn main() {
    // Collecte initiale
    let snapshot = match collect_snapshot() {
        Ok(s)  => s,
        Err(e) => { eprintln!("Erreur : {}", e); return; }
    };

    let shared_snapshot = Arc::new(Mutex::new(snapshot));

    // Thread de rafraîchissement toutes les 5 secondes
    {
        let shared_snapshot = Arc::clone(&shared_snapshot);
        thread::spawn(move || loop {
            thread::sleep(Duration::from_secs(5));
            match collect_snapshot() {
                Ok(new_snap) => {
                    let mut lock = shared_snapshot.lock().unwrap();
                    *lock = new_snap;
                }
                Err(e) => eprintln!("Erreur de rafraîchissement : {}", e),
            }
        });
    }

    // Serveur TCP
    let listener = TcpListener::bind("0.0.0.0:7878").expect("Impossible de binder le port 7878");
    println!("Serveur en écoute sur le port 7878...");
    println!("Connecte-toi avec : nc localhost 7878");

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                let shared_snapshot = Arc::clone(&shared_snapshot);
                thread::spawn(move || handle_client(stream, shared_snapshot));
            }
            Err(e) => eprintln!("Erreur : {}", e),
        }
    }
}