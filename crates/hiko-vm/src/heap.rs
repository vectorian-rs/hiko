use crate::value::{GcRef, HeapObject};

pub struct Heap {
    objects: Vec<Option<HeapObject>>,
    marks: Vec<bool>,
    free_list: Vec<u32>,
    alloc_since_gc: usize,
    gc_threshold: usize,
    max_objects: Option<usize>,
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

impl Heap {
    pub fn new() -> Self {
        Heap {
            objects: Vec::with_capacity(4096),
            marks: Vec::with_capacity(4096),
            free_list: Vec::new(),
            alloc_since_gc: 0,
            gc_threshold: 1024,
            max_objects: None,
        }
    }

    pub fn set_max_objects(&mut self, max: usize) {
        self.max_objects = Some(max);
    }

    pub fn alloc(&mut self, obj: HeapObject) -> GcRef {
        if let Some(max) = self.max_objects {
            let live = self.objects.len() - self.free_list.len();
            if live >= max {
                panic!("heap limit exceeded: {live} objects (max {max})");
            }
        }
        self.alloc_since_gc += 1;
        let idx = if let Some(idx) = self.free_list.pop() {
            self.objects[idx as usize] = Some(obj);
            idx
        } else {
            let idx = self.objects.len() as u32;
            self.objects.push(Some(obj));
            self.marks.push(false);
            idx
        };
        GcRef(idx)
    }

    pub fn get(&self, r: GcRef) -> Result<&HeapObject, &'static str> {
        self.objects
            .get(r.0 as usize)
            .and_then(|slot| slot.as_ref())
            .ok_or("dangling GcRef")
    }

    pub fn should_collect(&self) -> bool {
        self.alloc_since_gc >= self.gc_threshold
    }

    /// Mark a single ref. Returns true if it was newly marked.
    fn mark(&mut self, r: GcRef) -> bool {
        let idx = r.0 as usize;
        if self.marks[idx] {
            return false;
        }
        self.marks[idx] = true;
        true
    }

    /// Run mark-and-sweep. `roots` is an iterator of all root GcRefs.
    pub fn collect(&mut self, roots: impl Iterator<Item = GcRef>) {
        for m in self.marks.iter_mut() {
            *m = false;
        }

        // Worklist avoids stack overflow on deep object graphs
        let mut worklist: Vec<GcRef> = Vec::new();
        let mut children: Vec<GcRef> = Vec::new();

        for r in roots {
            if self.mark(r) {
                worklist.push(r);
            }
        }

        while let Some(r) = worklist.pop() {
            children.clear();
            if let Some(obj) = self.objects[r.0 as usize].as_ref() {
                obj.for_each_gc_ref(|c| children.push(c));
            }
            for &child in &children {
                if self.mark(child) {
                    worklist.push(child);
                }
            }
        }

        self.free_list.clear();
        for i in 0..self.objects.len() {
            if self.objects[i].is_some() && !self.marks[i] {
                self.objects[i] = None;
                self.free_list.push(i as u32);
            }
        }

        self.alloc_since_gc = 0;
        let live_count = self.objects.len() - self.free_list.len();
        self.gc_threshold = (live_count * 2).max(1024);
    }

    pub fn live_count(&self) -> usize {
        self.objects.iter().filter(|o| o.is_some()).count()
    }
}
