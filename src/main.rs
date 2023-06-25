use std::cell::RefCell;
use std::pin::Pin;
use breadx::display::DisplayConnection;
use futures::channel::oneshot;
use futures::prelude::*;
use signal_hook::consts::signal::SIGINT;
use signal_hook_tokio::Signals;
use crate::init_keyboard::reinit_loop;

mod config;
mod init_keyboard;

#[tokio::main]
async fn main() {
    let config = match config::load_config() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Failed to load config: {}", err);
            return;
        }
    };

    let signals = Signals::new([SIGINT]).expect("failed to register signal handler");
    let (interrupt_tx, interrupt_rx) = oneshot::channel();
    let signal_handler_task = handle_signals(signals, interrupt_tx);

    let conn = RefCell::new(
        DisplayConnection::connect(None)
            .expect("failed to connect to X server"));

    let local_set = tokio::task::LocalSet::new();
    let init_keyboard_task: Pin<Box<dyn Future<Output=()>>> = if let Some(init_keyboard) = &config.init_keyboard {
        Box::pin(local_set.run_until(reinit_loop(&conn, init_keyboard)))
    } else {
        Box::pin(future::ready(()))
    };

    // Wait for SIGINT.
    tokio::select! {
        _ = interrupt_rx => {
            return;
        }
        _ = init_keyboard_task => {}
        _ = signal_handler_task => {}
    }
}

async fn handle_signals(mut signals: Signals, interrupt_tx: oneshot::Sender<()>) {
    let mut interrupt_tx = Some(interrupt_tx);
    while let Some(signal) = signals.next().await {
        match signal {
            SIGINT => {
                if let Some(tx) = interrupt_tx {
                    tx.send(()).unwrap();
                    interrupt_tx = None;    // Can only send once since send() consumes the sender
                }
            }
            _ => unreachable!(),
        }
    }
}
