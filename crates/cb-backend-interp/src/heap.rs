use std::cell::RefCell;
use std::rc::Rc;

use cb_diagnostics::Symbol;
use cb_ir::types::IrType;
use cb_ir::{StructDefInfo, TypeDefId};
use cb_runtime_sys::CbStringApi;

use crate::value::{Value, default_value};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TypeInstanceId {
    pub index: u32,
    pub generation: u32,
}

pub struct TypeInstanceObj {
    pub type_def: TypeDefId,
    pub fields: Vec<Value>,
    pub prev: Option<TypeInstanceId>,
    pub next: Option<TypeInstanceId>,
    pub is_sentinel: bool,
}

pub struct Slab {
    entries: Vec<Option<TypeInstanceObj>>,
    generations: Vec<u32>,
    free_list: Vec<u32>,
}

impl Default for Slab {
    fn default() -> Self {
        Self::new()
    }
}

impl Slab {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            generations: Vec::new(),
            free_list: Vec::new(),
        }
    }

    pub fn alloc(&mut self, obj: TypeInstanceObj) -> TypeInstanceId {
        if let Some(idx) = self.free_list.pop() {
            let generation = self.generations[idx as usize];
            self.entries[idx as usize] = Some(obj);
            TypeInstanceId { index: idx, generation }
        } else {
            let idx = self.entries.len() as u32;
            self.entries.push(Some(obj));
            self.generations.push(0);
            TypeInstanceId { index: idx, generation: 0 }
        }
    }

    pub fn get(&self, id: TypeInstanceId) -> Option<&TypeInstanceObj> {
        if self.generations.get(id.index as usize).copied() != Some(id.generation) {
            return None;
        }
        self.entries[id.index as usize].as_ref()
    }

    pub fn get_mut(&mut self, id: TypeInstanceId) -> Option<&mut TypeInstanceObj> {
        if self.generations.get(id.index as usize).copied() != Some(id.generation) {
            return None;
        }
        self.entries[id.index as usize].as_mut()
    }

    pub fn free(&mut self, id: TypeInstanceId) {
        debug_assert_eq!(
            self.generations[id.index as usize], id.generation,
            "stale TypeInstanceId in free()"
        );
        self.entries[id.index as usize] = None;
        self.generations[id.index as usize] = id.generation.wrapping_add(1);
        self.free_list.push(id.index);
    }
}

pub struct TypeList {
    pub sentinel: TypeInstanceId,
    pub tail: Option<TypeInstanceId>,
}

impl TypeList {
    pub fn new(slab: &mut Slab, type_def: TypeDefId) -> Self {
        let sentinel = slab.alloc(TypeInstanceObj {
            type_def,
            fields: Vec::new(),
            prev: None,
            next: None,
            is_sentinel: true,
        });
        TypeList {
            sentinel,
            tail: None,
        }
    }

    pub fn append(&mut self, slab: &mut Slab, id: TypeInstanceId) {
        let prev_id = self.tail.unwrap_or(self.sentinel);
        slab.get_mut(id).expect("append: entry must exist").prev = Some(prev_id);
        slab.get_mut(id).expect("append: entry must exist").next = None;
        slab.get_mut(prev_id).expect("append: prev must exist").next = Some(id);
        self.tail = Some(id);
    }

    pub fn unlink(&mut self, slab: &mut Slab, id: TypeInstanceId) {
        let entry = slab.get(id).expect("unlink: entry must exist");
        let prev = entry.prev;
        let next = entry.next;

        if let Some(prev_id) = prev {
            slab.get_mut(prev_id).expect("unlink: prev must exist").next = next;
        }
        if let Some(next_id) = next {
            slab.get_mut(next_id).expect("unlink: next must exist").prev = prev;
        }

        if self.tail == Some(id) {
            let new_tail = prev.filter(|&p| p != self.sentinel);
            self.tail = new_tail;
        }

        let entry = slab.get_mut(id).expect("unlink: entry must exist");
        entry.prev = None;
        entry.next = None;
    }

    pub fn first(&self, slab: &Slab) -> Option<TypeInstanceId> {
        slab.get(self.sentinel).expect("first: sentinel must exist").next
    }
}

#[derive(Debug)]
pub struct ArrayObj {
    pub dims: Vec<usize>,
    pub data: Vec<Value>,
    pub elem_type: IrType,
}

/// Failure constructing an array: the requested element count overflows
/// `usize` or cannot be allocated. The interpreter turns this into a clean
/// `RuntimeError` rather than aborting the process on a hostile size.
#[derive(Debug)]
pub struct ArrayAllocError;

impl ArrayObj {
    pub fn new(
        dims: Vec<usize>,
        elem_type: IrType,
        struct_defs: &[StructDefInfo],
        string_api: &'static CbStringApi,
    ) -> Result<Self, ArrayAllocError> {
        // Use checked multiplication so an overflowing product (e.g. from a
        // dimension that wrapped a negative value) is caught instead of
        // silently wrapping, and `try_reserve` so an over-large but
        // non-overflowing length fails cleanly instead of aborting.
        let mut total: usize = 1;
        for &d in &dims {
            total = total.checked_mul(d).ok_or(ArrayAllocError)?;
        }
        let default = default_value(&elem_type, struct_defs, string_api);
        let mut data: Vec<Value> = Vec::new();
        data.try_reserve_exact(total).map_err(|_| ArrayAllocError)?;
        data.resize(total, default);
        Ok(ArrayObj {
            dims,
            data,
            elem_type,
        })
    }

    pub fn flat_index(&self, indices: &[usize]) -> Option<usize> {
        if indices.len() != self.dims.len() {
            return None;
        }
        let mut idx = 0usize;
        let mut stride = 1usize;
        for i in (0..indices.len()).rev() {
            if indices[i] >= self.dims[i] {
                return None;
            }
            idx += indices[i] * stride;
            stride *= self.dims[i];
        }
        Some(idx)
    }

    pub fn total_len(&self) -> usize {
        self.data.len()
    }

    pub fn dim_len(&self, dim: usize) -> Option<usize> {
        self.dims.get(dim).copied()
    }
}

#[derive(Clone, Debug)]
pub struct StructObj {
    pub struct_name: Symbol,
    pub fields: Vec<(Symbol, Value)>,
}

pub type ArrayRef = Rc<RefCell<ArrayObj>>;
