use std::thread;

use banker::Banker;

mod banker;

const NUM_LOOP: usize = 100000;

fn main() {
    // リソース全体は 左箸1本と右箸1本、2人の哲学者が1本ずつ必要としている
    let banker = Banker::<2, 2>::new([1, 1], [[1, 1], [1, 1]]);
    let banker0 = banker.clone();

    let philosopher0 = thread::spawn(move || {
        for i in 0..NUM_LOOP {
            while !banker0.take(0, 0) {}
            while !banker0.take(0, 1) {}

            println!("0: eating {i} th food");

            banker0.release(0, 0);
            banker0.release(0, 1);
        }
    });

    let philosopher1 = thread::spawn(move || {
        for i in 0..NUM_LOOP {
            while !banker.take(1, 1) {}
            while !banker.take(1, 0) {}

            println!("1: eating {i} th food");

            banker.release(1, 1);
            banker.release(1, 0);
        }
    });

    philosopher0.join().unwrap();
    philosopher1.join().unwrap();
}
