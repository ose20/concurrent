use std::sync::{Arc, Mutex};

// 銀行家のアルゴリズム
#[derive(Debug)]
struct Resource<const NUM_RESOURCES: usize, const NUM_THREADS: usize> {
    // 利用可能なリソース
    available_resource: [usize; NUM_RESOURCES],
    // スレッドごとの確保中のリソース
    allocation_for_threads: [[usize; NUM_RESOURCES]; NUM_THREADS],
    // 各スレッドが必要とするリソースの最大値
    needed_for_threads: [[usize; NUM_RESOURCES]; NUM_THREADS],
}

impl<const NUM_RESOURCES: usize, const NUM_THREADS: usize> Resource<NUM_RESOURCES, NUM_THREADS> {
    fn new(
        available_resource: [usize; NUM_RESOURCES],
        needed_for_threads: [[usize; NUM_RESOURCES]; NUM_THREADS],
    ) -> Self {
        Resource {
            available_resource,
            allocation_for_threads: [[0; NUM_RESOURCES]; NUM_THREADS],
            needed_for_threads,
        }
    }

    // 現在の状態がデッドロック or 飢餓状態に陥らないか
    fn is_safe(&self) -> bool {
        // 各スレッドがリソース取得と解放に成功したか
        let mut finish = [false; NUM_THREADS];
        // 利用可能なリソースのシミュレート値
        let mut available_resource = self.available_resource;

        loop {
            // すべてのスレッドの中から、そのスレッドが求めるリソースを割り当てることができるかを検証する
            // 割り当てが完了したスレッドはその後リソースを全部解放する & self.allocation_for_thread は [0, ..., 0] じゃないので、
            // 0..NUM_THREADS の順番に見ると取りこぼしがあるため、ループをしている

            // このループで条件を満たすスレッドをいくつ見つけることができたか
            let mut num_true = 0;

            // 注意
            // finish は loop の外で持ってるので、前の loop で finish になったスレッドのリソースは
            // 解放されない、、、と思いきや、available_resource も loop の外で持ってるのでそんなことはなかった

            for (i, aloc) in self.allocation_for_threads.iter().enumerate() {
                // aloc: i 番目のスレッドへのリソース割り当て状況
                if finish[i] {
                    // finish してるスレッドのリソースは解放済みなので、continue しなくてはいけない
                    // してもいいではない
                    num_true += 1;
                    continue;
                }

                let need_rest_by_i = self.needed_for_threads[i]
                    .iter()
                    .zip(aloc)
                    .map(|(m, a)| m - a);
                let is_available = available_resource
                    .iter()
                    .zip(need_rest_by_i)
                    .all(|(w, n)| *w >= n);
                // println!("i = {i}, is_available = {is_available}");
                if is_available {
                    num_true += 1;
                    finish[i] = true;
                    // 必要なリソースをすべて借り切ったので、すべて返却する
                    for (available, alocation) in available_resource.iter_mut().zip(aloc) {
                        *available += *alocation;
                    }
                    break;
                    // これ本には書いてあるけど書くとバグる（書かないとバグらないわけではないので別の原因かも）
                }
            }

            match num_true {
                0 => return false,
                num if num == NUM_THREADS => return true,
                _ => continue,
            }
        }
    }

    // thread_id 番目のスレッドが resource_id 番目のリソースを必要単位取得可能か
    fn take(&mut self, thread_id: usize, resource_id: usize) -> bool {
        assert!(thread_id < NUM_THREADS && resource_id < NUM_RESOURCES);

        let res = self.needed_for_threads[thread_id][resource_id]
            - self.allocation_for_threads[thread_id][resource_id];

        if cfg!(debug_assertions) {
            println!("res: {res}");
            println!(
                "available_resource: {}",
                self.available_resource[resource_id]
            );
        }

        // 既に割り当てが完了している場合
        if res == 0 {
            return true;
        }

        // 必要量が割り当て可能なリソース量を超過している場合
        if self.available_resource[resource_id] < res {
            return false;
        }

        // リソースの割り当てをして、それが safe な状態かチェックする
        self.available_resource[resource_id] -= res;
        self.allocation_for_threads[thread_id][resource_id] += res;

        if cfg!(debug_assertions) {
            println!("before is_safe check {:?}", self);
        };
        if self.is_safe() {
            if cfg!(debug_assertions) {
                println!("after take: {:?}", self.available_resource);
            }
            true
        } else {
            // 遷移先が safe 状態じゃなかったので、状態を戻す
            self.allocation_for_threads[thread_id][resource_id] -= res;
            self.available_resource[resource_id] += res;
            if cfg!(debug_assertions) {
                println!("after take: {:?}", self.available_resource);
            }
            false
        }
    }

    fn release(&mut self, t_id: usize, r_id: usize) {
        assert!(t_id < NUM_THREADS && r_id < NUM_RESOURCES);

        let res = self.allocation_for_threads[t_id][r_id];
        self.allocation_for_threads[t_id][r_id] -= res;
        self.available_resource[r_id] += res;
        if cfg!(debug_assertions) {
            println!("after release: {:?}", self.available_resource);
        }
    }
}

#[derive(Clone)]
pub struct Banker<const NUM_RESOURCES: usize, const NUM_THREADS: usize> {
    resource: Arc<Mutex<Resource<NUM_RESOURCES, NUM_THREADS>>>,
}

impl<const NUM_RESOURCES: usize, const NUM_THREADS: usize> Banker<NUM_RESOURCES, NUM_THREADS> {
    pub fn new(
        available: [usize; NUM_RESOURCES],
        needed_for_threads: [[usize; NUM_RESOURCES]; NUM_THREADS],
    ) -> Self {
        Banker {
            resource: Arc::new(Mutex::new(Resource::new(available, needed_for_threads))),
        }
    }

    pub fn take(&self, t_id: usize, r_id: usize) -> bool {
        let mut r = self.resource.lock().unwrap();
        r.take(t_id, r_id)
    }

    pub fn release(&self, t_id: usize, r_id: usize) {
        let mut r = self.resource.lock().unwrap();
        r.release(t_id, r_id);
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_is_safe() {
        let resource = Resource {
            available_resource: [0, 1],
            allocation_for_threads: [[1, 0], [0, 0]],
            needed_for_threads: [[1, 1], [1, 1]],
        };

        assert!(resource.is_safe())
    }

    #[test]
    fn test_is_safe2() {
        let resource = Resource {
            available_resource: [0, 1],
            allocation_for_threads: [[0, 0], [1, 0]],
            needed_for_threads: [[1, 1], [1, 1]],
        };

        assert!(resource.is_safe())
    }
}
