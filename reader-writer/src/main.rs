mod channel;
mod semaphore;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use channel::channel;
use semaphore::Semaphore;

const NUM_LOOP: usize = 100000;
const NUM_THREADS: usize = 8;

fn main() {
    let (tx, rx) = channel(4);
    let mut v = Vec::new();

    let t = std::thread::spawn(move || {
        let mut cnt = 0;
        while cnt < NUM_THREADS * NUM_LOOP {
            let n = rx.recv();
            println!("recv: n = {:?}", n);
            cnt += 1;
        }
    });

    v.push(t);

    for i in 0..NUM_THREADS {
        let tx0 = tx.clone();
        let t = std::thread::spawn(move || {
            for j in 0..NUM_LOOP {
                tx0.send((i, j))
            }
        });
        v.push(t);
    }

    for t in v {
        t.join().unwrap();
    }
}
