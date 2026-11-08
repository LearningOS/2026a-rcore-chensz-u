use alloc::{collections::BTreeMap, vec::Vec};

#[derive(Default)]
pub(crate) struct ResourceTable {
    available: Vec<usize>,
    allocations: BTreeMap<(usize, usize), usize>,
    requests: BTreeMap<usize, usize>,
}

impl ResourceTable {
    pub(crate) fn add(&mut self, id: usize, available: usize) {
        self.available.resize(self.available.len().max(id + 1), 0);
        self.available[id] = available;
    }

    pub(crate) fn request(&mut self, tid: usize, id: usize, detect: bool) -> bool {
        self.requests.insert(tid, id);
        if detect && self.available[id] == 0 && !self.can_finish() {
            self.requests.remove(&tid);
            return false;
        }
        true
    }

    fn can_finish(&self) -> bool {
        let mut work = self.available.clone();
        let mut waiting = self.requests.clone();
        for (&(tid, id), &count) in &self.allocations {
            if !waiting.contains_key(&tid) {
                work[id] += count;
            }
        }
        while let Some(tid) = waiting
            .iter()
            .find_map(|(&tid, &id)| (work[id] > 0).then_some(tid))
        {
            waiting.remove(&tid);
            for (&(_, id), &count) in self.allocations.range((tid, 0)..=(tid, usize::MAX)) {
                work[id] += count;
            }
        }
        waiting.is_empty()
    }

    pub(crate) fn acquire(&mut self, tid: usize) {
        if let Some(id) = self.requests.remove(&tid) {
            self.available[id] -= 1;
            *self.allocations.entry((tid, id)).or_default() += 1;
        }
    }

    pub(crate) fn release(&mut self, tid: usize, id: usize, next: Option<usize>, mutex: bool) {
        if mutex {
            self.allocations.retain(|&(_, resource), _| resource != id);
            self.available[id] = 1;
        } else {
            if let Some(count) = self.allocations.get_mut(&(tid, id)) {
                *count -= 1;
                if *count == 0 {
                    self.allocations.remove(&(tid, id));
                }
            }
            self.available[id] += 1;
        }
        if let Some(next) = next {
            self.acquire(next);
        }
    }

    pub(crate) fn thread_exited(&mut self, tid: usize) {
        self.allocations.retain(|&(owner, _), _| owner != tid);
        self.requests.remove(&tid);
    }
}
