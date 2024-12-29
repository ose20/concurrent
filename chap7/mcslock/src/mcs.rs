use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};
use std::ptr::null_mut;
use std::sync::atomic::{fence, AtomicBool, AtomicPtr, Ordering};

// メモリオーダー
// Relaxed: 制約なし
// Acquire: この命令以降のメモリ読み書き命令が、この命令より先に実行されないことを保証。メモリ読み込み命令に指定可能
// Release: この命令より前のメモリ読み書き命令が、この命令より後に実行されないことを保証。メモリ書き込み命令に指定可能
// AcqRel: 読み込みの場合は Acquire で、書き込みの場合は Release となる
// SecCst: 前後のメモリ読み書き命令の順序を維持

// 各スレッドはこの last 変数に対してアトミックにリンクリストのノードを追加していく
pub struct MCSLock<T> {
    last: AtomicPtr<MCSNode<T>>, // キューの最後尾
    data: UnsafeCell<T>,
}

// リンクリスト用のノード型
// ロックを獲得する際は locked を true にし、他のスレッドによって false に設定されるまでスピンする
pub struct MCSNode<T> {
    next: AtomicPtr<MCSNode<T>>, // 次のノード
    locked: AtomicBool,          // true ならロック獲得(試行?)中
}

pub struct MCSLockGuard<'a, T> {
    node: &'a mut MCSNode<T>, // 自スレッドのノード
    mcs_lock: &'a MCSLock<T>, // キューの最後尾と保護対象データへの参照
}

// スレッド間のデータ共有と、チャネルを使っ送受信が可能と設定
unsafe impl<T> Sync for MCSLock<T> {}
unsafe impl<T> Send for MCSLock<T> {}

impl<T> MCSNode<T> {
    pub fn new() -> Self {
        MCSNode {
            next: AtomicPtr::new(null_mut()),
            locked: AtomicBool::new(false),
        }
    }
}

// 保護対象データの immutable な参照外し
impl<'a, T> Deref for MCSLockGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.mcs_lock.data.get() }
    }
}

// 保護対象データの mutable な参照外し
impl<'a, T> DerefMut for MCSLockGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.mcs_lock.data.get() }
    }
}

impl<T> MCSLock<T> {
    pub fn new(v: T) -> Self {
        MCSLock {
            last: AtomicPtr::new(null_mut()),
            data: UnsafeCell::new(v),
        }
    }

    // lock を獲得する側で MCSNode::new() で作ったものを渡す想定?
    // じゃあこっちで吸収できないのか？みたいな疑問が当然沸き...
    pub fn lock<'a>(&'a self, node: &'a mut MCSNode<T>) -> MCSLockGuard<T> {
        // 自スレッド用のノードを初期化
        // MCSNode::new() で作ったものが渡されている場合は既にされてる
        node.next = AtomicPtr::new(null_mut());
        node.locked = AtomicBool::new(false);

        let guard = MCSLockGuard {
            node,
            mcs_lock: self,
        };

        // 自身をキューの最後尾とする
        let ptr = guard.node as *mut MCSNode<T>;
        // 既存の最後尾を prev とする
        let prev = self.last.swap(ptr, Ordering::Relaxed);

        // 最後尾が null の場合は誰もロックを獲得しようとしていないためロック獲得
        // null 以外の場合は、自身をキューの最後尾に追加
        if !prev.is_null() {
            // 今回の node をロック獲得中と設定
            guard.node.locked.store(true, Ordering::Relaxed);

            // 自身をキューの最後尾に追加
            let prev = unsafe { &*prev };
            prev.next.store(ptr, Ordering::Relaxed);

            // 他のスレッドから false に設定されるまでスピン
            while guard.node.locked.load(Ordering::Relaxed) {}
        }

        fence(Ordering::Acquire);
        // guard が返れば、deref で普通に値がとれる
        guard
    }
}

// ロックの解除とはすなわち
// lock で確保した型が Drop される時の挙動を定義することだ
impl<'a, T> Drop for MCSLockGuard<'a, T> {
    fn drop(&mut self) {
        // 自身の次のノードが null かつ自身が最後尾のノードなら、最後尾を null に設定
        if self.node.next.load(Ordering::Relaxed).is_null() {
            let ptr = self.node as *mut MCSNode<T>;
            // ↓で Err になるときは、↑の if 文評価後から↓の if 文評価の間に他のスレッドによって last が追加された場合
            if self
                .mcs_lock
                .last
                .compare_exchange(ptr, null_mut(), Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
        }

        // 自身の次のスレッドが Lock 関数実行中なので、その終了を待機
        // ロック獲得待機中のスレッドが必ずいるので、この while loop は必ず終わるはず
        while self.node.next.load(Ordering::Relaxed).is_null() {}
        let next = unsafe { &mut *self.node.next.load(Ordering::Relaxed) };
        next.locked.store(false, Ordering::Release);
    }
}
