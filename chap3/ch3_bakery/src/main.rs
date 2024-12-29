use std::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};
use std::sync::atomic::{fence, Ordering};
use std::thread;

const NUM_THREADS: usize = 4;
const NUM_LOOP: usize = 100000;

macro_rules! read_mem {
    ($addr: expr) => {
        unsafe { read_volatile($addr) }
    };
}

macro_rules! write_mem {
    ($addr: expr, $val: expr) => {
        unsafe { write_volatile($addr, $val) }
    };
}

struct BakeryLock {
    entering: [bool; NUM_THREADS],
    tickets: [Option<u64>; NUM_THREADS],
}

impl BakeryLock {
    fn lock(&mut self, idx: usize) -> LockGuard {
        // entering[idx] は、ticket を取得中であることを示すために true にする
        fence(Ordering::SeqCst);
        write_mem!(&mut self.entering[idx], true);
        fence(Ordering::SeqCst);

        let max = (0..NUM_THREADS)
            .map(|idx| read_mem!(&self.tickets[idx]).unwrap_or(0))
            .max()
            .unwrap_or(0);

        let ticket = max + 1;
        write_mem!(&mut self.tickets[idx], Some(ticket));

        fence(Ordering::SeqCst);
        write_mem!(&mut self.entering[idx], false);
        fence(Ordering::SeqCst);

        // ここから待機処理 <9>
        for i in 0..NUM_THREADS {
            if i == idx {
                continue;
            }

            // スレッドiがチケット取得中なら待機
            // entering[idx] を false から true -> false にする流れの中に待機処理はないからこの待機で Dead Lock は起きない
            // でも ticket 番号がかぶっちゃうことはない？
            // 4つの fence がキーワードっぽい
            while read_mem!(&self.entering[i]) {} // <10>

            while let Some(t) = read_mem!(&self.tickets[i]) {
                // この 2 つ目の条件を消すとプログラムが止まらなくなる
                // つまり、ticket == t && idx < i がなりたつケースがある
                // え、ticket 番号かぶってるじゃん
                // fence は ticket がかぶらないようにするための処理ではないということになる
                if ticket < t || (ticket == t && idx < i) {
                    break;
                }
            }
        }

        fence(Ordering::SeqCst);
        LockGuard { idx }
    }
}

// ロック管理用の型
struct LockGuard {
    idx: usize,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        fence(Ordering::SeqCst);
        write_mem!(&mut LOCK.tickets[self.idx], None);
    }
}

static mut LOCK: BakeryLock = BakeryLock {
    entering: [false; NUM_THREADS],
    tickets: [None; NUM_THREADS],
};

static mut COUNT: u64 = 0;

fn main() {
    let mut v = Vec::new();
    for i in 0..NUM_THREADS {
        let th = thread::spawn(move || {
            for _ in 0..NUM_LOOP {
                // ここ lock 関数の末尾に ; つけるとプログラムが壊れる
                let _lock = unsafe { LOCK.lock(i) };
                unsafe {
                    let c = read_volatile(addr_of!(COUNT));
                    write_volatile(addr_of_mut!(COUNT), c + 1);
                }
            }
        });
        v.push(th);
    }

    for th in v {
        th.join().unwrap();
    }

    println!(
        "COUNT = {} (expected = {})",
        unsafe { COUNT },
        NUM_LOOP * NUM_THREADS
    );
}
