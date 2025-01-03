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
    lock_ver: Vec<AtomicU64>, // ストライプに対する lock & verson
    global_clock: AtomicU64,  // global version-clock

    // アドレスからストライプ番号に変換するシフト量
    // ストライプサイズが1バイトならメモリとストライプは1対1なのでシフト量0
    // ストライプサイズが2バイトなら、アドレスを2で割った値がストライプ番号のため、シフト量は1
    // ストライプサイズが8バイトなら、シフト量は3
    // あれ、ストライプサイズを const でもってるからシフト量は3で固定じゃないの？
    // ストライプとメモリのサイズは 2^n じゃないといけないらしい
    shift_size: u32,
}

impl Memory {
    pub fn new() -> Self {
        // メモリ領域を生成
        let mem = [0].repeat(MEM_SIZE);

        // アドレスからストライプ番号へ変換するシフト量を計算
        // ストライプのサイズは 2^n にアラインメントされている必要あり
        // 2進表示したときに、末尾に連続する 0 の数
        // STRIP_SIZE は 8 なので shift = 3 になる
        // ほら、計算するまでもないじゃん
        let shift = STRIPE_SIZE.trailing_zeros();

        // lock&version を初期化
        let mut lock_ver = Vec::new();

        // MEM_SIZE >> shift
        // メモリサイズをストライプサイズで割ってることになる(ストライプが2冪の場合)
        for _ in 0..MEM_SIZE >> shift {
            lock_ver.push(AtomicU64::new(0));
        }

        Memory {
            mem,
            lock_ver,
            global_clock: AtomicU64::new(0),
            shift_size: shift,
        }
    }

    // global version-clock をインクリメント
    fn inc_global_clock(&mut self) -> u64 {
        self.global_clock.fetch_add(1, Ordering::AcqRel)
    }

    // 対象のアドレスのバージョンを取得
    fn get_addr_ver(&self, addr: usize) -> u64 {
        // 対応するストライプの index かな？
        let idx = addr >> self.shift_size;
        let n = self.lock_ver[idx].load(Ordering::Relaxed);
        // ロックをしているかいなかを最上位ビットで管理しているので、バージョンだけを取り出すために最上位を消してる
        n & !(1 << 63)
    }

    // 対象のアドレスのバージョンが rv 以下でロックされていないかをテスト
    // ↑これは、rv 以下かつロックされていないという方が正しいと思われる
    // なぜなら、ロックされていて、かつ rv 以上のバージョンだと true が返ってしまう
    // rv はバージョンで、最上位ビットが 0 だと仮定すれば、そもそもロック取ってるならその時点で false
    fn test_not_modify(&self, addr: usize, rv: u64) -> bool {
        let idx = addr >> self.shift_size;
        let n = self.lock_ver[idx].load(Ordering::Relaxed);
        // ロックのビットは最上位ビットとするため、
        // 単に rv と比較するだけでテスト可能
        n <= rv
    }

    // 対象アドレスのロックを獲得
    fn lock_addr(&mut self, addr: usize) -> bool {
        let idx = addr >> self.shift_size;
        self.lock_ver[idx]
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |val| {
                // 最上位ビットの値をテスト & セット
                let n = val & (1 << 63);
                if n == 0 {
                    Some(val | (1 << 63))
                } else {
                    None
                }
            })
            .is_ok()
    }

    // 対象アドレスのロックを解放
    fn unlock_addr(&mut self, addr: usize) {
        let idx = addr >> self.shift_size;
        self.lock_ver[idx].fetch_add(!(1 << 63), Ordering::Relaxed);
    }
}

pub struct ReadTrans<'a> {
    read_ver: u64,  // read-version
    is_abort: bool, // 競合を検知した場合に true
    mem: &'a Memory,
}

impl<'a> ReadTrans<'a> {
    fn new(mem: &'a Memory) -> Self {
        ReadTrans {
            is_abort: false,
            // global version-clock 読み込み
            read_ver: mem.global_clock.load(Ordering::Acquire),

            mem,
        }
    }

    // メモリ読み込み関数
    pub fn load(&mut self, addr: usize) -> Option<[u8; STRIPE_SIZE]> {
        // 競合を検知した場合に終了
        if self.is_abort {
            return None;
        }

        // アドレスがストライプのアラインメントに沿っているかチェック
        // ストライプサイズが 2^n なので、addr の下位 n ビットあg 0 であることを確認している
        assert_eq!(addr & (STRIPE_SIZE - 1), 0);

        // 読み込みメモリがロックされておらず、read-version 以下か判定
        if !self.mem.test_not_modify(addr, self.read_ver) {
            self.is_abort = true;
            return None;
        }

        fence(Ordering::Acquire);

        // メモリ読み込み。単なるコピー
        let mut mem = [0; STRIPE_SIZE];
        for (dst, src) in mem
            .iter_mut()
            .zip(self.mem.mem[addr..addr + STRIPE_SIZE].iter())
        {
            *dst = *src;
        }

        fence(Ordering::SeqCst);

        // 読み込みメモリがロックされておらず、read-version 以下か判定
        if !self.mem.test_not_modify(addr, self.read_ver) {
            self.is_abort = true;
            return None;
        }

        Some(mem)
    }
}

pub struct WriteTrans<'a> {
    read_ver: u64,                                // read-version
    read_set: HashSet<usize>,                     // read-set
    write_set: HashMap<usize, [u8; STRIPE_SIZE]>, // write-set
    locked: Vec<usize>,                           // ロック済みアドレス
    is_abort: bool,                               // 競合を検知した場合に真
    mem: &'a mut Memory,                          // Memoryへの参照
}

impl<'a> Drop for WriteTrans<'a> {
    fn drop(&mut self) {
        // ロック済みアドレスのロックを解除
        for addr in self.locked.iter() {
            self.mem.unlock_addr(*addr);
        }
    }
}

impl<'a> WriteTrans<'a> {
    fn new(mem: &'a mut Memory) -> Self {
        WriteTrans {
            read_set: HashSet::new(),
            write_set: HashMap::new(),
            locked: Vec::new(),
            is_abort: false,
            // global version-clock読み込み
            // あれ、少なくとも global_clock はこのスコープ内ではここしかないけどオーダリング厳しくする必要ある?
            // Acquire: この命令以降のメモリ読み書き命令が、この命令より先に実行されないことを保証。メモリ読み込み命令に指定可能
            read_ver: mem.global_clock.load(Ordering::Acquire),

            mem,
        }
    }

    // メモリ書き込み関数
    pub fn store(&mut self, addr: usize, val: [u8; STRIPE_SIZE]) {
        // アドレスがストライプのアラインメントに沿っているかチェック
        assert_eq!(addr & (STRIPE_SIZE - 1), 0);
        self.write_set.insert(addr, val);
    }

    // メモリ読み込み関数
    pub fn load(&mut self, addr: usize) -> Option<[u8; STRIPE_SIZE]> {
        // 競合を検知したら終了
        if self.is_abort {
            return None;
        }

        // アドレスがストライプのアラインメントに沿っているかチェック
        assert_eq!(addr & (STRIPE_SIZE - 1), 0);

        // 読み込みアドレスを保存
        self.read_set.insert(addr);

        // write-set にあればそれを読み込み
        if let Some(m) = self.write_set.get(&addr) {
            return Some(*m);
        }

        // 読み込みメモリがロックされておらず、read-version以下か判定
        if !self.mem.test_not_modify(addr, self.read_ver) {
            self.is_abort = true;
            return None;
        }

        fence(Ordering::Acquire);

        // メモリ読み込み。単なるコピー
        let mut mem = [0; STRIPE_SIZE];
        for (dst, src) in mem
            .iter_mut()
            .zip(self.mem.mem[addr..addr + STRIPE_SIZE].iter())
        {
            *dst = *src;
        }

        fence(Ordering::SeqCst);

        // 読み込みメモリがロックされておらず、read-version以下か判定
        if !self.mem.test_not_modify(addr, self.read_ver) {
            self.is_abort = true;
            return None;
        }

        Some(mem)
    }

    // write-set 中のアドレスをロック
    // すべてのアドレスをロックで獲得できた場合は真をリターンする
    fn lock_write_set(&mut self) -> bool {
        for (addr, _) in self.write_set.iter() {
            if self.mem.lock_addr(*addr) {
                // ロックが獲得できた場合は、locked に追加
                self.locked.push(*addr);
            } else {
                // できなかった場合は false を返す
                return false;
            }
        }
        true
    }

    // read-set の検証
    fn validate_read_set(&self) -> bool {
        for addr in self.read_set.iter() {
            // write-set 中にあるアドレスの場合は
            // 自スレッドがロック獲得しているはず
            if self.write_set.contains_key(addr) {
                // バージョンのみ検査
                let ver = self.mem.get_addr_ver(*addr);
                if ver > self.read_ver {
                    return false;
                }
            } else {
                // 他スレッドがロックしてないかバージョンチェック
                if !self.mem.test_not_modify(*addr, self.read_ver) {
                    return false;
                }
            }
        }
        true
    }

    // コミット
    fn commit(&mut self, ver: u64) {
        // すべてのアドレスに対する書き込み。単なるメモリコピー
        for (addr, val) in self.write_set.iter() {
            let addr = *addr;
            for (dst, src) in self.mem.mem[addr..addr + STRIPE_SIZE].iter_mut().zip(val) {
                *dst = *src
            }
        }

        fence(Ordering::Release);

        // すべてのアドレスのロック解除 & バージョン更新
        for (addr, _) in self.write_set.iter() {
            let idx = addr >> self.mem.shift_size;
            self.mem.lock_ver[idx].store(ver, Ordering::Relaxed);
        }

        // ロック済みアド絵rす集合をクリア
        self.locked.clear();
    }
}

pub enum STMResult<T> {
    Ok(T),
    Retry, // トランザクションをリトライ
    Abort, // トランザクションを中止
}

pub struct STM {
    mem: UnsafeCell<Memory>, // 実際のメモリ
}

// スレッド間で共有可能に設定。チャネルで送受信可能に設定
unsafe impl Sync for STM {}
unsafe impl Send for STM {}

impl STM {
    pub fn new() -> Self {
        STM {
            mem: UnsafeCell::new(Memory::new()),
        }
    }

    // 読み込みトランザクション
    pub fn read_transaction<F, R>(&self, f: F) -> Option<R>
    where
        F: Fn(&mut ReadTrans) -> STMResult<R>,
    {
        loop {
            // 1. global version-clock 読み込み
            let mut tr = ReadTrans::new(unsafe { &*self.mem.get() });

            // 2. 投機的実行
            match f(&mut tr) {
                STMResult::Abort => return None, // 中断
                STMResult::Retry => {
                    if tr.is_abort {
                        continue; // リトライ
                    }
                    return None; // 中断
                }
                STMResult::Ok(val) => {
                    if tr.is_abort {
                        continue; // リトライ
                    } else {
                        return Some(val); // 3. こミット
                    }
                }
            }
        }
    }

    // 書き込みトランザクション
    pub fn write_transaction<F, R>(&self, f: F) -> Option<R>
    where
        F: Fn(&mut WriteTrans) -> STMResult<R>,
    {
        loop {
            // 1. global version-clock 読み込み
            let mut tr = WriteTrans::new(unsafe { &mut *self.mem.get() });

            // 2. 投機的実行
            let result;
            match f(&mut tr) {
                STMResult::Abort => return None,
                STMResult::Retry => {
                    if tr.is_abort {
                        continue;
                    }
                    return None;
                }
                STMResult::Ok(val) => {
                    if tr.is_abort {
                        continue;
                    }
                    result = val;
                }
            }
            todo!()
        }
    }
}
