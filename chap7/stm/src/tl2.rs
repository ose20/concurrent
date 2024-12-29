use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::atomic::{fence, AtomicU64, Ordering};

// ストライプのサイズ
const STRIPE_SIZE: usize = 8; // u64, 8 バイト

// メモリの合計サイズ
// このため 512 / 8 = 64 個のストライプを使用可能
const MEM_SIZE: usize = 512; // 512 バイト

pub struct Memory {
    mem: Vec<u8>,             // メモリ
    lock_var: Vec<AtomicU64>, // ストライプに対する lock & verson
    global_clock: AtomicU64,  // global version-clock

    // アドレスからストライプ番号に変換するシフト量
    // ストライプサイズが1バイトならメモリとストライプは1対1なのでシフト量0
    // ストライプサイズが2バイトなら、アドレスを2で割った値がストライプ番号のため、シフト量は1
    // ストライプサイズが8バイトなら、シフト量は3
    // あれ、ストライプサイズを const でもってるからシフト量は3で固定じゃないの？
    // ストライプとメモリのサイズは 2^n じゃないといけないらしい
    shift_size: u32,
}
