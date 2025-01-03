use core::panic;
use std::sync::Arc;
use std::{thread, time};

mod tl2;

// メモリ読み込みようのマクロ
#[macro_export]
macro_rules! load {
    ($t:ident, $a:expr) => {
        if let Some(v) = ($t).load($a) {
            v
        } else {
            // 読み込みに失敗したらリトライ
            return tl2::STMResult::Retry;
        }
    };
}

// メモリ書き込み用マクロ
#[macro_export]
macro_rules! store {
    ($t:ident, $a:expr, $v:expr) => {
        $t.store($a, $v)
    };
}

// 哲学者の数
const NUM_PHILOSOPHERS: usize = 8;

// 箸一本にたいして STM のストライプを1つ用いる

fn philosopher(stm: Arc<tl2::STM>, n: usize) {
    // 左と右の箸用のメモリ
    let left = 8 * n;
    let right = 8 * ((n + 1) % NUM_PHILOSOPHERS);

    #[allow(clippy::blocks_in_conditions)]
    for _ in 0..500000 {
        // 箸を取り上げる
        while !stm
            .write_transaction(|tr| {
                let mut f1 = load!(tr, left); // 左の箸
                let mut f2 = load!(tr, right); // 右の箸
                if f1[0] == 0 && f2[0] == 0 {
                    // 両方空いていれば 1 に設定
                    f1[0] = 1;
                    f2[0] = 1;
                    store!(tr, left, f1);
                    store!(tr, right, f2);
                    tl2::STMResult::Ok(true)
                } else {
                    // 両方取れない場合は取得失敗
                    tl2::STMResult::Ok(false)
                }
            })
            .unwrap()
        {}

        // 箸をおく
        stm.write_transaction(|tr| {
            let mut f1 = load!(tr, left);
            let mut f2 = load!(tr, right);
            f1[0] = 0;
            f2[0] = 0;
            store!(tr, left, f1);
            store!(tr, right, f2);
            tl2::STMResult::Ok(())
        });
    }
}

// 哲学者を観測する観測者のコード
fn observer(stm: Arc<tl2::STM>) {
    for _ in 0..10000 {
        let chopsticks = stm
            .read_transaction(|tr| {
                let mut v = [0; NUM_PHILOSOPHERS];
                for i in 0..NUM_PHILOSOPHERS {
                    v[i] = load!(tr, 8 * i)[0];
                }

                tl2::STMResult::Ok(v)
            })
            .unwrap();

        println!("{:?}", chopsticks);

        // 取り上げられている橋が奇数の場合は不正
        let mut n = 0;
        for c in &chopsticks {
            if *c == 1 {
                n += 1;
            }
        }

        if n & 1 != 0 {
            panic!("inconsistent")
        }

        // 100 マイクロ秒スリープ
        let us = time::Duration::from_micros(100);
        thread::sleep(us);
    }
}

fn main() {
    println!("Hello, world!");
}
