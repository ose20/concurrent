use nix::sys::mman::{mprotect, ProtFlags};
use std::alloc::{alloc, dealloc, Layout};
use std::collections::{HashMap, HashSet, LinkedList};
use std::ffi::c_void;
use std::ptr;

// すべてのスレッド終了時に戻ってくる先 <1>
static mut CTX_MAIN: Option<Box<Registers>> = None;

// 不要なスタック領域 <2>
static mut UNUSED_STACK: (*mut u8, Layout) = (ptr::null_mut(), Layout::new::<u8>());

// スレッドの実行キュー <3>
static mut CONTEXTS: LinkedList<Box<Context>> = LinkedList::new();

// スレッドIDの集合 <4>
static mut ID: *mut HashSet<u64> = ptr::null_mut();

// メッセージキュー <1>
static mut MESSAGES: *mut MappedList<u64> = ptr::null_mut();

// 待機スレッド集合 <2>
static mut WAITING: *mut HashMap<u64, Box<Context>> = ptr::null_mut();

// # Callee-saved vs Caller-saved
// x86_64 arch には 16 個の汎用レジスタがあり、そのうち、次の6つは Callee-saved (呼び出された側が保存する)
// rdx, rbp, r12, r13, r14, r15
// これらは関数呼び出しの後でも値が維持されるべきレジスタ
// # Caller-saved (呼び出し元が保存する)
// rax, rcx, rdx, rsi, rbi, r8, r9, r10, r11
// 関数呼び出し時に破壊されても問題がないレジスタで、呼び出し元が必要に応じて保存する必要がある

// グリーンスレッドのコンテキストスイッチでは、次のスレッドに切り替わる前に、現在のスレッドの Callee-saved レジスタを保存する必要がある
// なので Registers がフィールドとして持ってる
// 他のフィールドは？
// - rsp (スタックポインタ)
// - rdi (引数や一時的データの一時保存用)
// 他の省かれたレジスタは、関数呼び出し時に破壊される可能性があるため、スレッドの切り替え時には特に保存する必要がない

// 物理的なメモリの順番がフィールド定義の順番と同じになるように repr(C) がいる
#[repr(C)] // <1>
struct Registers {
    rbx: u64, // 引数として渡した場合、(%rdi)
    rbp: u64, // 同じく、8(%rdi)
    r12: u64, // 同じく、16(%rdi)
    r13: u64,
    r14: u64,
    r15: u64,
    rsp: u64,
    rdx: u64,
}

impl Registers {
    // <3>
    fn new(rsp: u64) -> Self {
        Registers {
            rbx: 0,
            rbp: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rsp,
            rdx: entry_point as u64, // <4>
        }
    }
}

// ctx というポインタを引数にしてる
// これは、アセンブリ内で %rdi レジスタを通じてアクセスできる
// switch_context の返り値の型が never 型になっているのは、アセンブリの定義を見ると話kる
// ret で return せず、jmp 命令で終わっている
extern "C" {
    fn set_context(ctx: *mut Registers) -> u64;
    fn switch_context(ctx: *const Registers) -> !;
}

// スレッド開始時に実行する関数の型
type Entry = fn(); // <1>

// ページサイズ。Linuxだと4KiB
const PAGE_SIZE: usize = 4 * 1024; // 4KiB <2>

struct MappedList<T> {
    // <1>
    map: HashMap<u64, LinkedList<T>>,
}

impl<T> MappedList<T> {
    fn new() -> Self {
        MappedList {
            map: HashMap::new(),
        }
    }

    // keyに対応するリストの最後尾に追加 <2>
    fn push_back(&mut self, key: u64, val: T) {
        if let Some(list) = self.map.get_mut(&key) {
            // 対応するリストが存在するなら追加
            list.push_back(val);
        } else {
            // 存在しない場合、新たにリストを作成して追加
            let mut list = LinkedList::new();
            list.push_back(val);
            self.map.insert(key, list);
        }
    }

    // keyに対応するリストの一番前から取り出す <3>
    fn pop_front(&mut self, key: u64) -> Option<T> {
        if let Some(list) = self.map.get_mut(&key) {
            let val = list.pop_front();
            if list.len() == 0 {
                self.map.remove(&key);
            }
            val
        } else {
            None
        }
    }

    fn clear(&mut self) {
        self.map.clear();
    }
}

// コンテキスト <3>
struct Context {
    regs: Registers,      // レジスタ
    stack: *mut u8,       // スタック
    stack_layout: Layout, // スタックレイアウト
    entry: Entry,         // エントリポイント
    id: u64,              // スレッドID
}

impl Context {
    // レジスタ情報へのポインタを取得
    fn get_regs_mut(&mut self) -> *mut Registers {
        &mut self.regs as *mut Registers
    }

    fn get_regs(&self) -> *const Registers {
        &self.regs as *const Registers
    }

    #[inline(never)]
    fn new(func: Entry, stack_size: usize, id: u64) -> Self {
        // <4>
        // スタック領域の確保 <5>
        // アライメントは、メモリの先頭が特定の倍数であることを保証するためのもの
        let layout = Layout::from_size_align(stack_size, PAGE_SIZE).unwrap();
        let stack = unsafe { alloc(layout) };

        // ガードページの設定 <6>
        // 今回は PROT_NONE なのｄ
        // stack から始まる PAGE_SIZE 分の領域のメモリに対して、すべてのアクセス（読み書き実行）を禁止している
        unsafe { mprotect(stack as *mut c_void, PAGE_SIZE, ProtFlags::PROT_NONE).unwrap() };

        // レジスタの初期化 <7>
        let regs = Registers::new(stack as u64 + stack_size as u64);

        // コンテキストの初期化
        Context {
            regs: regs,
            stack: stack,
            stack_layout: layout,
            entry: func,
            id: id,
        }
    }
}

fn get_id() -> u64 {
    loop {
        let rnd = rand::random::<u64>(); // <1>
        unsafe {
            if !(*ID).contains(&rnd) {
                // <2>
                (*ID).insert(rnd); // <3>
                return rnd;
            };
        }
    }
}

pub fn spawn(func: Entry, stack_size: usize) -> u64 {
    // <1>
    unsafe {
        let id = get_id(); // <2>
        CONTEXTS.push_back(Box::new(Context::new(func, stack_size, id))); // <3>
        schedule(); // <4>
        id // <5>
    }
}

pub fn schedule() {
    unsafe {
        // 実行可能なプロセスが自身のみであるため即座にリターン <1>
        if CONTEXTS.len() == 1 {
            return;
        }

        // 自身のコンテキストを実行キューの最後に移動
        let mut ctx = CONTEXTS.pop_front().unwrap(); // <2>
                                                     // レジスタ保存領域へのポインタを取得 <3>
        let regs = ctx.get_regs_mut();
        CONTEXTS.push_back(ctx);

        // レジスタを保存 <4>
        if set_context(regs) == 0 {
            // 次のスレッドにコンテキストスイッチ
            let next = CONTEXTS.front().unwrap();
            switch_context((**next).get_regs());
        }

        // 不要なスタック領域を削除
        rm_unused_stack(); // <5>
    }
}

#[no_mangle]
pub extern "C" fn entry_point() {
    unsafe {
        // 指定されたエントリ関数を実行 <1>
        let ctx = CONTEXTS.front().unwrap();
        (ctx.entry)();

        // 以降がスレッド終了時の後処理

        // 自身のコンテキストを取り除く
        let ctx = CONTEXTS.pop_front().unwrap();

        // スレッドIDを削除
        (*ID).remove(&ctx.id);

        // 不要なスタック領域として保存
        // この段階で解放すると、以降のコードでスタックが使えなくなる
        // ので、context_switch 後に呼び出す
        UNUSED_STACK = ((*ctx).stack, (*ctx).stack_layout); // <2>

        match CONTEXTS.front() {
            // <3>
            Some(c) => {
                // 次のスレッドにコンテキストスイッチ
                switch_context((**c).get_regs());
            }
            None => {
                // すべてのスレッドが終了した場合、main関数のスレッドに戻る
                if let Some(c) = &CTX_MAIN {
                    switch_context(&**c as *const Registers);
                }
            }
        };
    }
    panic!("entry_point"); // <4>
}

pub fn spawn_from_main(func: Entry, stack_size: usize) {
    unsafe {
        // すでに初期化済みならエラーとする
        if let Some(_) = &CTX_MAIN {
            panic!("spawn_from_main is called twice");
        }

        // main関数用のコンテキストを生成
        CTX_MAIN = Some(Box::new(Registers::new(0)));
        if let Some(ctx) = &mut CTX_MAIN {
            // グローバル変数を初期化 <1>
            let mut msgs = MappedList::new();
            MESSAGES = &mut msgs as *mut MappedList<u64>;

            let mut waiting = HashMap::new();
            WAITING = &mut waiting as *mut HashMap<u64, Box<Context>>;

            let mut ids = HashSet::new();
            ID = &mut ids as *mut HashSet<u64>;

            // すべてのスレッド終了時の戻り先を保存 <2>
            if set_context(&mut **ctx as *mut Registers) == 0 {
                // 最初に起動するスレッドのコンテキストを生成して実行 <3>
                CONTEXTS.push_back(Box::new(Context::new(func, stack_size, get_id())));
                let first = CONTEXTS.front().unwrap();
                switch_context(first.get_regs());
            }

            // 不要なスタックを解放 <4>
            rm_unused_stack();

            // グローバル変数をクリア
            CTX_MAIN = None;
            CONTEXTS.clear();
            MESSAGES = ptr::null_mut();
            WAITING = ptr::null_mut();
            ID = ptr::null_mut();

            msgs.clear(); // <5>
            waiting.clear();
            ids.clear();
        }
    }
}

unsafe fn rm_unused_stack() {
    if UNUSED_STACK.0 != ptr::null_mut() {
        // スタック領域の保護を解除 <1>
        mprotect(
            UNUSED_STACK.0 as *mut c_void,
            PAGE_SIZE,
            ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
        )
        .unwrap();
        // スタック領域解放 <2>
        dealloc(UNUSED_STACK.0, UNUSED_STACK.1);
        UNUSED_STACK = (ptr::null_mut(), Layout::new::<u8>());
    }
}

pub fn send(key: u64, msg: u64) {
    // <1>
    unsafe {
        // メッセージキューの最後尾に追加
        (*MESSAGES).push_back(key, msg);

        // スレッドが受信待ちの場合に実行キューに移動
        if let Some(ctx) = (*WAITING).remove(&key) {
            CONTEXTS.push_back(ctx);
        }
    }
    schedule(); // <2>
}

pub fn recv() -> Option<u64> {
    unsafe {
        // スレッドIDを取得
        let key = CONTEXTS.front().unwrap().id;

        // メッセージがすでにキューにある場合即座にリターン
        if let Some(msg) = (*MESSAGES).pop_front(key) {
            return Some(msg);
        }

        // 実行可能なスレッドが他にいない場合はデッドロック
        if CONTEXTS.len() == 1 {
            panic!("deadlock");
        }

        // 実行中のスレッドを受信待ち状態に移行
        let mut ctx = CONTEXTS.pop_front().unwrap();
        let regs = ctx.get_regs_mut();
        (*WAITING).insert(key, ctx);

        // 次の実行可能なスレッドにコンテキストスイッチ
        if set_context(regs) == 0 {
            let next = CONTEXTS.front().unwrap();
            switch_context((**next).get_regs());
        }

        // 不要なスタックを削除
        rm_unused_stack();

        // 受信したメッセージを取得
        (*MESSAGES).pop_front(key)
    }
}
