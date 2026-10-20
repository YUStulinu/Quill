//! Built-in functions: their types, the run-time symbols that implement them, and whether they are pure.

use crate::types::{Scheme, Ty};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Builtin {
    Print,
    Println,
    PrintInt,
    IntToString,
    StringLength,
    StringGet,
    Substring,
    CharToString,
    StringToInt,
    ArrayMake,
    ArrayGet,
    ArraySet,
    ArrayLength,
    Panic,
    Assert,
    ClockMs,
    GcCollect,
    GcCount,
    // Not callable by name; used by lowering.
    Concat,
    Equal,
}

pub struct BuiltinInfo {
    pub builtin: Builtin,
    pub name: &'static str,
    /// The C symbol in the run-time library.
    pub symbol: &'static str,
    pub doc: &'static str,
    /// No side effects and no failure: calls with unused results may be deleted.
    pub pure: bool,
}

pub const BUILTINS: &[BuiltinInfo] = &[
    BuiltinInfo { builtin: Builtin::Print, name: "print", symbol: "quill_print", doc: "Writes a string to standard output.", pure: false },
    BuiltinInfo {
        builtin: Builtin::Println,
        name: "println",
        symbol: "quill_println",
        doc: "Writes a string and a newline to standard output.",
        pure: false,
    },
    BuiltinInfo {
        builtin: Builtin::PrintInt,
        name: "print_int",
        symbol: "quill_print_int",
        doc: "Writes an integer and a newline to standard output.",
        pure: false,
    },
    BuiltinInfo {
        builtin: Builtin::IntToString,
        name: "int_to_string",
        symbol: "quill_int_to_string",
        doc: "The decimal representation of an integer.",
        pure: true,
    },
    BuiltinInfo {
        builtin: Builtin::StringLength,
        name: "string_length",
        symbol: "quill_string_length",
        doc: "The length of a string in bytes.",
        pure: true,
    },
    BuiltinInfo {
        builtin: Builtin::StringGet,
        name: "string_get",
        symbol: "quill_string_get",
        doc: "The byte at an index (0-based); fails if out of range.",
        pure: false,
    },
    BuiltinInfo {
        builtin: Builtin::Substring,
        name: "substring",
        symbol: "quill_substring",
        doc: "`substring(s, start, len)`; fails if out of range.",
        pure: false,
    },
    BuiltinInfo {
        builtin: Builtin::CharToString,
        name: "char_to_string",
        symbol: "quill_char_to_string",
        doc: "A one-byte string from a byte value (0-255).",
        pure: false,
    },
    BuiltinInfo {
        builtin: Builtin::StringToInt,
        name: "string_to_int",
        symbol: "quill_string_to_int",
        doc: "Parses a decimal integer; fails if the string is not one.",
        pure: false,
    },
    BuiltinInfo {
        builtin: Builtin::ArrayMake,
        name: "array_make",
        symbol: "quill_array_make",
        doc: "`array_make(n, x)`: a new array of n copies of x.",
        pure: false,
    },
    BuiltinInfo {
        builtin: Builtin::ArrayGet,
        name: "array_get",
        symbol: "quill_array_get",
        doc: "The element at an index; fails if out of range.",
        pure: false,
    },
    BuiltinInfo {
        builtin: Builtin::ArraySet,
        name: "array_set",
        symbol: "quill_array_set",
        doc: "Replaces the element at an index; fails if out of range.",
        pure: false,
    },
    BuiltinInfo {
        builtin: Builtin::ArrayLength,
        name: "array_length",
        symbol: "quill_array_length",
        doc: "The number of elements of an array.",
        pure: true,
    },
    BuiltinInfo { builtin: Builtin::Panic, name: "panic", symbol: "quill_panic", doc: "Stops the program with a message.", pure: false },
    BuiltinInfo {
        builtin: Builtin::Assert,
        name: "assert",
        symbol: "quill_assert",
        doc: "Stops the program if the condition is false.",
        pure: false,
    },
    BuiltinInfo {
        builtin: Builtin::ClockMs,
        name: "clock_ms",
        symbol: "quill_clock_ms",
        doc: "Milliseconds of a monotonic clock.",
        pure: false,
    },
    BuiltinInfo {
        builtin: Builtin::GcCollect,
        name: "gc_collect",
        symbol: "quill_gc_collect",
        doc: "Runs the garbage collector now.",
        pure: false,
    },
    BuiltinInfo {
        builtin: Builtin::GcCount,
        name: "gc_count",
        symbol: "quill_gc_count",
        doc: "How many collections have run so far.",
        pure: false,
    },
    BuiltinInfo { builtin: Builtin::Concat, name: "++", symbol: "quill_concat", doc: "", pure: true },
    BuiltinInfo { builtin: Builtin::Equal, name: "==", symbol: "quill_equal", doc: "", pure: true },
];

pub fn info(b: Builtin) -> &'static BuiltinInfo {
    BUILTINS.iter().find(|i| i.builtin == b).expect("every builtin has an entry")
}

pub fn lookup(name: &str) -> Option<&'static BuiltinInfo> {
    BUILTINS.iter().find(|i| i.name == name && !matches!(i.builtin, Builtin::Concat | Builtin::Equal))
}

/// The type scheme of a builtin.
pub fn scheme(b: Builtin) -> Scheme {
    use Builtin::*;
    let f = |ps: Vec<Ty>, r: Ty| Ty::Fn(ps, Box::new(r));
    let a = Ty::Gen(0);
    match b {
        Print | Println => Scheme::mono(f(vec![Ty::string()], Ty::unit())),
        PrintInt => Scheme::mono(f(vec![Ty::int()], Ty::unit())),
        IntToString => Scheme::mono(f(vec![Ty::int()], Ty::string())),
        StringLength => Scheme::mono(f(vec![Ty::string()], Ty::int())),
        StringGet => Scheme::mono(f(vec![Ty::string(), Ty::int()], Ty::int())),
        Substring => Scheme::mono(f(vec![Ty::string(), Ty::int(), Ty::int()], Ty::string())),
        CharToString => Scheme::mono(f(vec![Ty::int()], Ty::string())),
        StringToInt => Scheme::mono(f(vec![Ty::string()], Ty::int())),
        ArrayMake => Scheme { vars: 1, ty: f(vec![Ty::int(), a.clone()], Ty::array(a)) },
        ArrayGet => Scheme { vars: 1, ty: f(vec![Ty::array(a.clone()), Ty::int()], a) },
        ArraySet => Scheme { vars: 1, ty: f(vec![Ty::array(a.clone()), Ty::int(), a], Ty::unit()) },
        ArrayLength => Scheme { vars: 1, ty: f(vec![Ty::array(a)], Ty::int()) },
        Panic => Scheme { vars: 1, ty: f(vec![Ty::string()], a) },
        Assert => Scheme::mono(f(vec![Ty::bool()], Ty::unit())),
        ClockMs | GcCount => Scheme::mono(f(vec![], Ty::int())),
        GcCollect => Scheme::mono(f(vec![], Ty::unit())),
        Concat => Scheme::mono(f(vec![Ty::string(), Ty::string()], Ty::string())),
        Equal => Scheme { vars: 1, ty: f(vec![a.clone(), a], Ty::bool()) },
    }
}

pub fn arity(b: Builtin) -> usize {
    match scheme(b).ty {
        Ty::Fn(ps, _) => ps.len(),
        _ => 0,
    }
}
