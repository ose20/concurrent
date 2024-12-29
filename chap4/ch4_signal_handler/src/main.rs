use libc::SIGUSR1;
use signal_hook::iterator::Signals;
use std::{error::Error, process, thread, time::Duration};

fn main() -> Result<(), Box<dyn Error>> {
    println!("pid: {}", process::id());

    let mut signals = Signals::new([SIGUSR1])?;
    thread::spawn(move || {
        for sig in signals.forever() {
            println!("received signal: {:?}", sig);
        }
    });

    thread::sleep(Duration::from_secs(10));
    Ok(())
}
