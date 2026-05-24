use std::cell::RefCell;
use std::rc::Rc;

use cb_diagnostics::Symbol;
use cb_ir::types::IrType;
use cb_ir::TypeDefId;

use crate::value::{Value, default_value};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TypeInstanceId(pub u32);

pub struct TypeInstanceObj {
    pub type_def: TypeDefId,
    pub fields: Vec<Value>,
    pub prev: Option<TypeInstanceId>,
    pub next: Option<TypeInstanceId>,
    pub freed: bool,
    pub is_sentinel: bool,
}

pub struct Slab {
    entries: Vec<Option<TypeInstanceObj>>,
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
            free_list: Vec::new(),
        }
    }

    pub fn alloc(&mut self, obj: TypeInstanceObj) -> TypeInstanceId {
        if let Some(idx) = self.free_list.pop() {
            self.entries[idx as usize] = Some(obj);
            TypeInstanceId(idx)
        } else {
            let idx = self.entries.len() as u32;
            self.entries.push(Some(obj));
            TypeInstanceId(idx)
        }
    }

    pub fn get(&self, id: TypeInstanceId) -> &TypeInstanceObj {
        self.entries[id.0 as usize].as_ref().expect("slab entry missing")
    }

    pub fn get_mut(&mut self, id: TypeInstanceId) -> &mut TypeInstanceObj {
        self.entries[id.0 as usize].as_mut().expect("slab entry missing")
    }

    pub fn free(&mut self, id: TypeInstanceId) {
        self.entries[id.0 as usize] = None;
        self.free_list.push(id.0);
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
            freed: false,
            is_sentinel: true,
        });
        TypeList {
            sentinel,
            tail: None,
        }
    }

    pub fn append(&mut self, slab: &mut Slab, id: TypeInstanceId) {
        let prev_id = self.tail.unwrap_or(self.sentinel);
        slab.get_mut(id).prev = Some(prev_id);
        slab.get_mut(id).next = None;
        slab.get_mut(prev_id).next = Some(id);
        self.tail = Some(id);
    }

    pub fn unlink(&mut self, slab: &mut Slab, id: TypeInstanceId) {
        let prev = slab.get(id).prev;
        let next = slab.get(id).next;

        if let Some(prev_id) = prev {
            slab.get_mut(prev_id).next = next;
        }
        if let Some(next_id) = next {
            slab.get_mut(next_id).prev = prev;
        }

        if self.tail == Some(id) {
            let new_tail = prev.filter(|&p| p != self.sentinel);
            self.tail = new_tail;
        }

        slab.get_mut(id).prev = None;
        slab.get_mut(id).next = None;
    }

    pub fn first(&self, slab: &Slab) -> Option<TypeInstanceId> {
        slab.get(self.sentinel).next
    }
}

#[derive(Debug)]
pub struct ArrayObj {
    pub dims: Vec<usize>,
    pub data: Vec<Value>,
    pub elem_type: IrType,
}

impl ArrayObj {
    pub fn new(dims: Vec<usize>, elem_type: IrType) -> Self {
        let total: usize = dims.iter().product();
        let default = default_value(&elem_type, &[]);
        let data = vec![default; total];
        ArrayObj {
            dims,
            data,
            elem_type,
        }
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
