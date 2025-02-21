pub trait Entry {
    fn hash(&self) -> usize;
}

struct Table<E>
where
    E: Eq + PartialEq + Clone + Copy + Entry,
{
    count: usize,
    capacity: usize,
    buckets: Vec<Option<(E, E)>>,
}

impl<E> Table<E>
where
    E: Eq + PartialEq + Clone + Copy + Entry,
{
    pub fn new() -> Self {
        Table {
            count: 0,
            capacity: 0,
            buckets: vec![None; 16],
        }
    }

    pub fn free(&mut self) {
        self.buckets.truncate(0);
        self.buckets.shrink_to(0);
    }

    pub fn insert(&mut self, key: E, value: E) {
        let index = (key.hash() % self.capacity) as usize;

        if let Some((k, v)) = self.buckets[index] {
            if k == key {
                self.buckets[index] = Some((key, value));
            } else {
                self.resize();
                self.insert(key, value);
            }
        } else {
            self.buckets[index] = Some((key, value));
            self.count += 1;
        }
    }

    pub fn get(&self, key: E) -> Option<&E> {
        let index = key.hash() % self.buckets.len();
        self.buckets[index].as_ref().map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, key: E) -> Option<&mut E> {
        let index = key.hash() % self.buckets.len();
        self.buckets[index].as_mut().map(|(_, v)| v)
    }

    fn resize(&mut self) {
        let new_size = self.capacity + 8;
        let mut buckets = vec![None; new_size];

        for bucket in self.buckets.drain(..) {
            if let Some((k, v)) = bucket {
                let index = k.hash() % new_size;
                buckets[index] = Some((k, v));
            }
        }

        std::mem::swap(&mut self.buckets, &mut buckets);
    }
}
