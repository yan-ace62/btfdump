use std::cmp::{max, min};
use std::ffi::CStr;
use std::fmt;
use std::mem::size_of;

use object::{Object, ObjectSection};
use scroll::Pread;
use scroll_derive::{IOread, IOwrite, Pread as DerivePread, Pwrite, SizeWith};

use crate::{btf_error, BtfError, BtfResult};

const BTF_MAGIC: u16 = 0xeB9F;
const BTF_VERSION: u8 = 1;

//const BTF_MAX_TYPE: u32 = 0xffff;
//const BTF_MAX_NAME_OFFSET: u32 = 0xffff;
//const BTF_MAX_VLEN: u32 = 0xffff;

//const BTF_MAX_NR_TYPES: u32 = 0x7fffffff;
//const BTF_MAX_STR_OFFSET: u32 = 0x7fffffff;

//const BTF_KIND_UNKN: u32 = 0;
const BTF_KIND_INT: u32 = 1;
const BTF_KIND_PTR: u32 = 2;
const BTF_KIND_ARRAY: u32 = 3;
const BTF_KIND_STRUCT: u32 = 4;
const BTF_KIND_UNION: u32 = 5;
const BTF_KIND_ENUM: u32 = 6;
const BTF_KIND_FWD: u32 = 7;
const BTF_KIND_TYPEDEF: u32 = 8;
const BTF_KIND_VOLATILE: u32 = 9;
const BTF_KIND_CONST: u32 = 10;
const BTF_KIND_RESTRICT: u32 = 11;
const BTF_KIND_FUNC: u32 = 12;
const BTF_KIND_FUNC_PROTO: u32 = 13;
const BTF_KIND_VAR: u32 = 14;
const BTF_KIND_DATASEC: u32 = 15;
//const BTF_KIND_MAX: u32 = 15;
//const NR_BTF_KINDS: u32 = BTF_KIND_MAX + 1;

const BTF_INT_SIGNED: u32 = 0b001;
const BTF_INT_CHAR: u32 = 0b010;
const BTF_INT_BOOL: u32 = 0b100;

const BTF_VAR_STATIC: u32 = 0;
const BTF_VAR_GLOBAL_ALLOCATED: u32 = 1;

#[repr(C)]
#[derive(Debug, Copy, Clone, DerivePread, Pwrite, IOread, IOwrite, SizeWith)]
struct btf_header {
    pub magic: u16,
    pub version: u8,
    pub flags: u8,
    pub hdr_len: u32,
    pub type_off: u32,
    pub type_len: u32,
    pub str_off: u32,
    pub str_len: u32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, DerivePread, Pwrite, IOread, IOwrite, SizeWith)]
struct btf_type {
    pub name_off: u32,
    pub info: u32,
    pub type_id: u32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, DerivePread, Pwrite, IOread, IOwrite, SizeWith)]
struct btf_enum {
    pub name_off: u32,
    pub val: i32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, DerivePread, Pwrite, IOread, IOwrite, SizeWith)]
struct btf_array {
    pub val_type_id: u32,
    pub idx_type_id: u32,
    pub nelems: u32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, DerivePread, Pwrite, IOread, IOwrite, SizeWith)]
struct btf_member {
    pub name_off: u32,
    pub type_id: u32,
    pub offset: u32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, DerivePread, Pwrite, IOread, IOwrite, SizeWith)]
struct btf_param {
    pub name_off: u32,
    pub type_id: u32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, DerivePread, Pwrite, IOread, IOwrite, SizeWith)]
struct btf_datasec_var {
    pub type_id: u32,
    pub offset: u32,
    pub size: u32,
}

const EMPTY: &'static str = "";
const ANON_NAME: &'static str = "<anon>";

fn disp_name(s: &str) -> &str {
    if s == "" {
        ANON_NAME
    } else {
        s
    }
}

#[derive(Debug, PartialEq)]
pub enum BtfIntEncoding {
    None,
    Signed,
    Char,
    Bool,
}

impl fmt::Display for BtfIntEncoding {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            BtfIntEncoding::None => write!(f, "none"),
            BtfIntEncoding::Signed => write!(f, "signed"),
            BtfIntEncoding::Char => write!(f, "char"),
            BtfIntEncoding::Bool => write!(f, "bool"),
        }
    }
}

#[derive(Debug)]
pub struct BtfInt {
    pub name: String,
    pub bits: u32,
    pub offset: u32,
    pub encoding: BtfIntEncoding,
}

impl fmt::Display for BtfInt {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "<{}> '{}' bits:{} off:{}",
            "INT",
            disp_name(&self.name),
            self.bits,
            self.offset
        )?;
        match self.encoding {
            BtfIntEncoding::None => (),
            _ => write!(f, " enc:{}", self.encoding)?,
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct BtfPtr {
    pub type_id: u32,
}

impl fmt::Display for BtfPtr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "<{}> --> [{}]", "PTR", self.type_id)
    }
}

#[derive(Debug)]
pub struct BtfArray {
    pub nelems: u32,
    pub idx_type_id: u32,
    pub val_type_id: u32,
}

impl fmt::Display for BtfArray {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "<{}> n:{} idx-->[{}] val-->[{}]",
            "ARRAY", self.nelems, self.idx_type_id, self.val_type_id
        )
    }
}

#[derive(Debug)]
pub struct BtfMember {
    pub name: String,
    pub type_id: u32,
    pub bit_offset: u32,
    pub bit_size: u8,
}

impl fmt::Display for BtfMember {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "'{}' off:{}", disp_name(&self.name), self.bit_offset)?;
        if self.bit_size != 0 {
            write!(f, " sz:{}", self.bit_size)?;
        }
        write!(f, " --> [{}]", self.type_id)
    }
}

#[derive(Debug)]
pub struct BtfStruct {
    pub name: String,
    pub sz: u32,
    pub members: Vec<BtfMember>,
}

impl fmt::Display for BtfStruct {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "<{}> '{}' sz:{} n:{}",
            "STRUCT",
            disp_name(&self.name),
            self.sz,
            self.members.len()
        )?;
        for i in 0..self.members.len() {
            write!(f, "\n\t#{:02} {}", i, self.members[i])?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct BtfUnion {
    pub name: String,
    pub sz: u32,
    pub members: Vec<BtfMember>,
}

impl fmt::Display for BtfUnion {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "<{}> '{}' sz:{} n:{}",
            "UNION",
            disp_name(&self.name),
            self.sz,
            self.members.len()
        )?;
        for i in 0..self.members.len() {
            write!(f, "\n\t#{:02} {}", i, self.members[i])?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct BtfEnumValue {
    pub name: String,
    pub value: i32,
}

impl fmt::Display for BtfEnumValue {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} = {}", disp_name(&self.name), self.value)
    }
}

#[derive(Debug)]
pub struct BtfEnum {
    pub name: String,
    pub sz_bits: u32,
    pub values: Vec<BtfEnumValue>,
}

impl fmt::Display for BtfEnum {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "<{}> '{}' sz:{} n:{}",
            "ENUM",
            disp_name(&self.name),
            self.sz_bits,
            self.values.len()
        )?;
        for i in 0..self.values.len() {
            write!(f, "\n\t#{:02} {}", i, self.values[i])?;
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq)]
pub enum BtfFwdKind {
    Struct,
    Union,
}

impl fmt::Display for BtfFwdKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            BtfFwdKind::Struct => write!(f, "struct"),
            BtfFwdKind::Union => write!(f, "union"),
        }
    }
}

#[derive(Debug)]
pub struct BtfFwd {
    pub name: String,
    pub kind: BtfFwdKind,
}

impl fmt::Display for BtfFwd {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "<{}> '{}' kind:{}",
            "FWD",
            disp_name(&self.name),
            self.kind
        )
    }
}

#[derive(Debug)]
pub struct BtfTypedef {
    pub name: String,
    pub type_id: u32,
}

impl fmt::Display for BtfTypedef {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "<{}> '{}' --> [{}]",
            "TYPEDEF",
            disp_name(&self.name),
            self.type_id
        )
    }
}

#[derive(Debug)]
pub struct BtfVolatile {
    pub type_id: u32,
}

impl fmt::Display for BtfVolatile {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "<{}> --> [{}]", "VOLATILE", self.type_id)
    }
}

#[derive(Debug)]
pub struct BtfConst {
    pub type_id: u32,
}

impl fmt::Display for BtfConst {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "<{}> --> [{}]", "CONST", self.type_id)
    }
}

#[derive(Debug)]
pub struct BtfRestrict {
    pub type_id: u32,
}

impl fmt::Display for BtfRestrict {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "<{}> --> [{}]", "RESTRICT", self.type_id)
    }
}

#[derive(Debug)]
pub struct BtfFunc {
    pub name: String,
    pub proto_type_id: u32,
}

impl fmt::Display for BtfFunc {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "<{}> '{}' --> [{}]",
            "FUNC",
            disp_name(&self.name),
            self.proto_type_id
        )
    }
}

#[derive(Debug)]
pub struct BtfFuncParam {
    pub name: String,
    pub type_id: u32,
}

impl fmt::Display for BtfFuncParam {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "'{}' --> [{}]", disp_name(&self.name), self.type_id)
    }
}

#[derive(Debug)]
pub struct BtfFuncProto {
    pub res_type_id: u32,
    pub params: Vec<BtfFuncParam>,
}

impl fmt::Display for BtfFuncProto {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "<{}> r-->[{}] n:{}",
            "FUNC_PROTO",
            self.res_type_id,
            self.params.len()
        )?;
        for i in 0..self.params.len() {
            write!(f, "\n\t#{:02} {}", i, self.params[i])?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum BtfVarKind {
    Static,
    GlobalAlloc,
}

impl fmt::Display for BtfVarKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            BtfVarKind::Static => write!(f, "static"),
            BtfVarKind::GlobalAlloc => write!(f, "global-alloc"),
        }
    }
}

#[derive(Debug)]
pub struct BtfVar {
    pub name: String,
    pub type_id: u32,
    pub kind: BtfVarKind,
}

impl fmt::Display for BtfVar {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "<{}> '{}' kind:{} --> [{}]",
            "VAR",
            disp_name(&self.name),
            self.kind,
            self.type_id
        )
    }
}

#[derive(Debug)]
pub struct BtfDatasecVar {
    pub type_id: u32,
    pub offset: u32,
    pub sz: u32,
}

impl fmt::Display for BtfDatasecVar {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "off:{} sz:{} --> [{}]",
            self.offset, self.sz, self.type_id
        )
    }
}

#[derive(Debug)]
pub struct BtfDatasec {
    pub name: String,
    pub sz: u32,
    pub vars: Vec<BtfDatasecVar>,
}

impl fmt::Display for BtfDatasec {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "<{}> '{}' sz:{} n:{}",
            "DATASEC",
            disp_name(&self.name),
            self.sz,
            self.vars.len()
        )?;
        for i in 0..self.vars.len() {
            write!(f, "\n\t#{:02} {}", i, self.vars[i])?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum BtfType {
    Void,
    Int(BtfInt),
    Ptr(BtfPtr),
    Array(BtfArray),
    Struct(BtfStruct),
    Union(BtfUnion),
    Enum(BtfEnum),
    Fwd(BtfFwd),
    Typedef(BtfTypedef),
    Volatile(BtfVolatile),
    Const(BtfConst),
    Restrict(BtfRestrict),
    Func(BtfFunc),
    FuncProto(BtfFuncProto),
    Var(BtfVar),
    Datasec(BtfDatasec),
}

impl fmt::Display for BtfType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            BtfType::Void => write!(f, "<{}>", "VOID"),
            BtfType::Int(t) => t.fmt(f),
            BtfType::Ptr(t) => t.fmt(f),
            BtfType::Array(t) => t.fmt(f),
            BtfType::Struct(t) => t.fmt(f),
            BtfType::Union(t) => t.fmt(f),
            BtfType::Enum(t) => t.fmt(f),
            BtfType::Fwd(t) => t.fmt(f),
            BtfType::Typedef(t) => t.fmt(f),
            BtfType::Volatile(t) => t.fmt(f),
            BtfType::Const(t) => t.fmt(f),
            BtfType::Restrict(t) => t.fmt(f),
            BtfType::Func(t) => t.fmt(f),
            BtfType::FuncProto(t) => t.fmt(f),
            BtfType::Var(t) => t.fmt(f),
            BtfType::Datasec(t) => t.fmt(f),
        }
    }
}

impl BtfType {
    pub fn kind(&self) -> BtfKind {
        match self {
            BtfType::Void => BtfKind::Void,
            BtfType::Int(_) => BtfKind::Int,
            BtfType::Ptr(_) => BtfKind::Ptr,
            BtfType::Array(_) => BtfKind::Array,
            BtfType::Struct(_) => BtfKind::Struct,
            BtfType::Union(_) => BtfKind::Union,
            BtfType::Enum(_) => BtfKind::Enum,
            BtfType::Fwd(_) => BtfKind::Fwd,
            BtfType::Typedef(_) => BtfKind::Typedef,
            BtfType::Volatile(_) => BtfKind::Volatile,
            BtfType::Const(_) => BtfKind::Const,
            BtfType::Restrict(_) => BtfKind::Restrict,
            BtfType::Func(_) => BtfKind::Func,
            BtfType::FuncProto(_) => BtfKind::FuncProto,
            BtfType::Var(_) => BtfKind::Var,
            BtfType::Datasec(_) => BtfKind::Datasec,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            BtfType::Void => EMPTY,
            BtfType::Int(t) => &t.name,
            BtfType::Ptr(_) => EMPTY,
            BtfType::Array(_) => EMPTY,
            BtfType::Struct(t) => &t.name,
            BtfType::Union(t) => &t.name,
            BtfType::Enum(t) => &t.name,
            BtfType::Fwd(t) => &t.name,
            BtfType::Typedef(t) => &t.name,
            BtfType::Volatile(_) => EMPTY,
            BtfType::Const(_) => EMPTY,
            BtfType::Restrict(_) => EMPTY,
            BtfType::Func(t) => &t.name,
            BtfType::FuncProto(_) => EMPTY,
            BtfType::Var(t) => &t.name,
            BtfType::Datasec(t) => &t.name,
        }
    }
}

#[derive(Debug, Eq, PartialEq, Copy, Clone, Hash)]
pub enum BtfKind {
    Void,
    Int,
    Ptr,
    Array,
    Struct,
    Union,
    Enum,
    Fwd,
    Typedef,
    Volatile,
    Const,
    Restrict,
    Func,
    FuncProto,
    Var,
    Datasec,
}

impl std::str::FromStr for BtfKind {
    type Err = BtfError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "void" => Ok(BtfKind::Void),
            "int" | "i" => Ok(BtfKind::Int),
            "ptr" | "p" => Ok(BtfKind::Ptr),
            "array" | "arr" | "a" => Ok(BtfKind::Array),
            "struct" | "s" => Ok(BtfKind::Struct),
            "union" | "u" => Ok(BtfKind::Union),
            "enum" | "e" => Ok(BtfKind::Enum),
            "fwd" => Ok(BtfKind::Fwd),
            "typedef" | "t" => Ok(BtfKind::Typedef),
            "volatile" => Ok(BtfKind::Volatile),
            "const" => Ok(BtfKind::Const),
            "restrict" => Ok(BtfKind::Restrict),
            "func_proto" | "funcproto" | "fnproto" | "fp" => Ok(BtfKind::FuncProto),
            "func" | "fn" => Ok(BtfKind::Func),
            "var" | "v" => Ok(BtfKind::Var),
            "datasec" => Ok(BtfKind::Datasec),
            _ => Err(BtfError::new_owned(format!(
                "unrecognized btf kind: '{}'",
                s
            ))),
        }
    }
}

#[derive(Debug)]
struct BtfHeader {
    pub flags: u8,
    pub hdr_len: usize,
    pub type_off: usize,
    pub type_len: usize,
    pub str_off: usize,
    pub str_len: usize,
}

#[derive(Debug)]
pub struct Btf {
    hdr: BtfHeader,
    endian: scroll::Endian,
    types: Vec<BtfType>,
    ptr_sz: u32,
}

impl Btf {
    pub fn ptr_sz(&self) -> u32 {
        self.ptr_sz
    }

    pub fn types(&self) -> &Vec<BtfType> {
        &self.types
    }

    pub fn type_by_id(&self, type_id: u32) -> &BtfType {
        &self.types[type_id as usize]
    }

    pub fn type_cnt(&self) -> u32 {
        self.types.len() as u32
    }

    pub fn get_size_of(&self, type_id: u32) -> u32 {
        match self.type_by_id(type_id) {
            BtfType::Void => 0,
            BtfType::Int(t) => (t.bits + 7) / 8,
            BtfType::Volatile(t) => self.get_size_of(t.type_id),
            BtfType::Const(t) => self.get_size_of(t.type_id),
            BtfType::Restrict(t) => self.get_size_of(t.type_id),
            BtfType::Ptr(_) => self.ptr_sz,
            BtfType::Array(t) => t.nelems * self.get_size_of(t.val_type_id),
            BtfType::FuncProto(_) => 0,
            BtfType::Struct(t) => t.sz,
            BtfType::Union(t) => t.sz,
            BtfType::Enum(t) => (t.sz_bits + 7) / 8,
            BtfType::Fwd(_) => 0,
            BtfType::Typedef(t) => self.get_size_of(t.type_id),
            BtfType::Func(_) => 0,
            BtfType::Var(_) => 0,
            BtfType::Datasec(t) => t.sz,
        }
    }

    pub fn get_align_of(&self, type_id: u32) -> u32 {
        match self.type_by_id(type_id) {
            BtfType::Void => 0,
            BtfType::Int(t) => min(self.ptr_sz, (t.bits + 7) / 8),
            BtfType::Volatile(t) => self.get_align_of(t.type_id),
            BtfType::Const(t) => self.get_align_of(t.type_id),
            BtfType::Restrict(t) => self.get_align_of(t.type_id),
            BtfType::Ptr(_) => self.ptr_sz,
            BtfType::Array(t) => self.get_align_of(t.val_type_id),
            BtfType::FuncProto(_) => 0,
            BtfType::Struct(t) => {
                let mut align = 1;
                for m in &t.members {
                    align = max(align, self.get_align_of(m.type_id));
                }
                align
            }
            BtfType::Union(t) => {
                let mut align = 1;
                for m in &t.members {
                    align = max(align, self.get_align_of(m.type_id));
                }
                align
            }
            BtfType::Enum(t) => min(self.ptr_sz, (t.sz_bits + 7) / 8),
            BtfType::Fwd(_) => 0,
            BtfType::Typedef(t) => self.get_align_of(t.type_id),
            BtfType::Func(_) => 0,
            BtfType::Var(_) => 0,
            BtfType::Datasec(_) => 0,
        }
    }

    pub fn load<'data>(elf: object::ElfFile<'data>) -> BtfResult<Btf> {
        let endian = if elf.is_little_endian() {
            scroll::LE
        } else {
            scroll::BE
        };
        let btf_section = elf
            .section_by_name(".BTF")
            .ok_or_else(|| Box::new(BtfError::new("No .BTF section found!")))?;
        let data = btf_section.data();

        let hdr = data.pread_with::<btf_header>(0, endian)?;
        if hdr.magic != BTF_MAGIC {
            return btf_error(format!("Invalid BTF magic: {}", hdr.magic));
        }
        if hdr.version != BTF_VERSION {
            return btf_error(format!(
                "Unsupported BTF version: {}, expect: {}",
                hdr.version, BTF_VERSION
            ));
        }

        let mut btf = Btf {
            endian: endian,
            hdr: BtfHeader {
                flags: hdr.flags,
                hdr_len: hdr.hdr_len as usize,
                type_off: hdr.type_off as usize,
                type_len: hdr.type_len as usize,
                str_off: hdr.str_off as usize,
                str_len: hdr.str_len as usize,
            },
            types: vec![BtfType::Void],
            ptr_sz: if elf.elf().is_64 { 8 } else { 4 },
        };

        let type_off = size_of::<btf_header>() + btf.hdr.type_off;
        let type_data = &data[type_off..type_off + btf.hdr.type_len];
        let str_off = size_of::<btf_header>() + btf.hdr.str_off;
        let str_data = &data[str_off..str_off + btf.hdr.str_len];
        let mut off: usize = 0;
        while off < btf.hdr.type_len {
            let t = btf.load_type(&type_data[off..], str_data)?;
            off += Btf::type_size(&t);
            btf.types.push(t);
        }

        Ok(btf)
    }

    fn type_size(t: &BtfType) -> usize {
        let common = size_of::<btf_type>();
        match t {
            BtfType::Void => 0,
            BtfType::Ptr(_)
            | BtfType::Fwd(_)
            | BtfType::Typedef(_)
            | BtfType::Volatile(_)
            | BtfType::Const(_)
            | BtfType::Restrict(_)
            | BtfType::Func(_) => common,
            BtfType::Int(_) | BtfType::Var(_) => common + size_of::<u32>(),
            BtfType::Array(_) => common + size_of::<btf_array>(),
            BtfType::Struct(t) => common + t.members.len() * size_of::<btf_member>(),
            BtfType::Union(t) => common + t.members.len() * size_of::<btf_member>(),
            BtfType::Enum(t) => common + t.values.len() * size_of::<btf_enum>(),
            BtfType::FuncProto(t) => common + t.params.len() * size_of::<btf_param>(),
            BtfType::Datasec(t) => common + t.vars.len() * size_of::<btf_datasec_var>(),
        }
    }

    fn load_type(&self, data: &[u8], strs: &[u8]) -> BtfResult<BtfType> {
        let t = data.pread_with::<btf_type>(0, self.endian)?;
        let extra = &data[size_of::<btf_type>()..];
        let kind = (t.info >> 24) & 0xf;
        match kind {
            BTF_KIND_INT => self.load_int(&t, extra, strs),
            BTF_KIND_PTR => Ok(BtfType::Ptr(BtfPtr { type_id: t.type_id })),
            BTF_KIND_ARRAY => self.load_array(extra),
            BTF_KIND_STRUCT => self.load_struct(&t, extra, strs),
            BTF_KIND_UNION => self.load_union(&t, extra, strs),
            BTF_KIND_ENUM => self.load_enum(&t, extra, strs),
            BTF_KIND_FWD => Ok(BtfType::Fwd(BtfFwd {
                name: Btf::get_btf_str(strs, t.name_off)?,
                kind: if Btf::get_kind(t.info) {
                    BtfFwdKind::Union
                } else {
                    BtfFwdKind::Struct
                },
            })),
            BTF_KIND_TYPEDEF => Ok(BtfType::Typedef(BtfTypedef {
                name: Btf::get_btf_str(strs, t.name_off)?,
                type_id: t.type_id,
            })),
            BTF_KIND_VOLATILE => Ok(BtfType::Volatile(BtfVolatile { type_id: t.type_id })),
            BTF_KIND_CONST => Ok(BtfType::Const(BtfConst { type_id: t.type_id })),
            BTF_KIND_RESTRICT => Ok(BtfType::Restrict(BtfRestrict { type_id: t.type_id })),
            BTF_KIND_FUNC => Ok(BtfType::Func(BtfFunc {
                name: Btf::get_btf_str(strs, t.name_off)?,
                proto_type_id: t.type_id,
            })),
            BTF_KIND_FUNC_PROTO => self.load_func_proto(&t, extra, strs),
            BTF_KIND_VAR => self.load_var(&t, extra, strs),
            BTF_KIND_DATASEC => self.load_datasec(&t, extra, strs),
            _ => btf_error(format!("Unknown BTF kind: {}", kind)),
        }
    }

    fn load_int(&self, t: &btf_type, extra: &[u8], strs: &[u8]) -> BtfResult<BtfType> {
        let info = extra.pread_with::<u32>(0, self.endian)?;
        let enc = (info >> 24) & 0xf;
        let off = (info >> 16) & 0xff;
        let bits = info & 0xff;
        Ok(BtfType::Int(BtfInt {
            name: Btf::get_btf_str(strs, t.name_off)?,
            bits: bits,
            offset: off,
            encoding: match enc {
                0 => BtfIntEncoding::None,
                BTF_INT_SIGNED => BtfIntEncoding::Signed,
                BTF_INT_CHAR => BtfIntEncoding::Char,
                BTF_INT_BOOL => BtfIntEncoding::Bool,
                _ => {
                    return btf_error(format!("Unknown BTF int encoding: {}", enc));
                }
            },
        }))
    }

    fn load_array(&self, extra: &[u8]) -> BtfResult<BtfType> {
        let info = extra.pread_with::<btf_array>(0, self.endian)?;
        Ok(BtfType::Array(BtfArray {
            nelems: info.nelems,
            idx_type_id: info.idx_type_id,
            val_type_id: info.val_type_id,
        }))
    }

    fn load_struct(&self, t: &btf_type, extra: &[u8], strs: &[u8]) -> BtfResult<BtfType> {
        Ok(BtfType::Struct(BtfStruct {
            name: Btf::get_btf_str(strs, t.name_off)?,
            sz: t.type_id, // it's a type/size union in C
            members: self.load_members(t, extra, strs)?,
        }))
    }

    fn load_union(&self, t: &btf_type, extra: &[u8], strs: &[u8]) -> BtfResult<BtfType> {
        Ok(BtfType::Union(BtfUnion {
            name: Btf::get_btf_str(strs, t.name_off)?,
            sz: t.type_id, // it's a type/size union in C
            members: self.load_members(t, extra, strs)?,
        }))
    }

    fn load_members(&self, t: &btf_type, extra: &[u8], strs: &[u8]) -> BtfResult<Vec<BtfMember>> {
        let mut res = Vec::new();
        let mut off: usize = 0;
        let bits = Btf::get_kind(t.info);

        for _ in 0..Btf::get_vlen(t.info) {
            let m = extra.pread_with::<btf_member>(off, self.endian)?;
            res.push(BtfMember {
                name: Btf::get_btf_str(strs, m.name_off)?,
                type_id: m.type_id,
                bit_size: if bits { (m.offset >> 24) as u8 } else { 0 },
                bit_offset: if bits { m.offset & 0xffffff } else { m.offset },
            });
            off += size_of::<btf_member>();
        }
        Ok(res)
    }

    fn load_enum(&self, t: &btf_type, extra: &[u8], strs: &[u8]) -> BtfResult<BtfType> {
        let mut vals = Vec::new();
        let mut off: usize = 0;

        for _ in 0..Btf::get_vlen(t.info) {
            let v = extra.pread_with::<btf_enum>(off, self.endian)?;
            vals.push(BtfEnumValue {
                name: Btf::get_btf_str(strs, v.name_off)?,
                value: v.val,
            });
            off += size_of::<btf_enum>();
        }
        Ok(BtfType::Enum(BtfEnum {
            name: Btf::get_btf_str(strs, t.name_off)?,
            sz_bits: t.type_id, // it's a type/size union in C
            values: vals,
        }))
    }

    fn load_func_proto(&self, t: &btf_type, extra: &[u8], strs: &[u8]) -> BtfResult<BtfType> {
        let mut params = Vec::new();
        let mut off: usize = 0;

        for _ in 0..Btf::get_vlen(t.info) {
            let p = extra.pread_with::<btf_param>(off, self.endian)?;
            params.push(BtfFuncParam {
                name: Btf::get_btf_str(strs, p.name_off)?,
                type_id: p.type_id,
            });
            off += size_of::<btf_param>();
        }
        Ok(BtfType::FuncProto(BtfFuncProto {
            res_type_id: t.type_id,
            params: params,
        }))
    }

    fn load_var(&self, t: &btf_type, extra: &[u8], strs: &[u8]) -> BtfResult<BtfType> {
        let kind = extra.pread_with::<u32>(0, self.endian)?;
        Ok(BtfType::Var(BtfVar {
            name: Btf::get_btf_str(strs, t.name_off)?,
            type_id: t.type_id,
            kind: match kind {
                BTF_VAR_STATIC => BtfVarKind::Static,
                BTF_VAR_GLOBAL_ALLOCATED => BtfVarKind::GlobalAlloc,
                _ => {
                    return btf_error(format!("Unknown BTF var kind: {}", kind));
                }
            },
        }))
    }

    fn load_datasec(&self, t: &btf_type, extra: &[u8], strs: &[u8]) -> BtfResult<BtfType> {
        let mut vars = Vec::new();
        let mut off: usize = 0;

        for _ in 0..Btf::get_vlen(t.info) {
            let v = extra.pread_with::<btf_datasec_var>(off, self.endian)?;
            vars.push(BtfDatasecVar {
                type_id: v.type_id,
                offset: v.offset,
                sz: v.size,
            });
            off += size_of::<btf_datasec_var>();
        }
        Ok(BtfType::Datasec(BtfDatasec {
            name: Btf::get_btf_str(strs, t.name_off)?,
            sz: t.type_id, // it's a type/size union in C
            vars: vars,
        }))
    }

    fn get_btf_str(strs: &[u8], off: u32) -> BtfResult<String> {
        let c_str = unsafe { CStr::from_ptr(&strs[off as usize] as *const u8 as *const i8) };
        Ok(c_str.to_str()?.to_owned())
    }

    fn get_vlen(info: u32) -> u32 {
        info & 0xffff
    }

    fn get_kind(info: u32) -> bool {
        (info >> 31) == 1
    }
}