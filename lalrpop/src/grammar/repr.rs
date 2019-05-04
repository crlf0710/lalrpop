//! Compiled representation of a grammar. Simplified, normalized
//! version of `parse_tree`. The normalization passes produce this
//! representation incrementally.

use collections::{map, Map};
use grammar::free_variables::FreeVariables;
use grammar::pattern::Pattern;
use message::Content;
use std::fmt::{Debug, Display, Error, Formatter};
use string_cache::DefaultAtom as Atom;
use util::Sep;

// These concepts we re-use wholesale
pub use grammar::parse_tree::{
    Annotation, InternToken, Lifetime, NonterminalString, Path, Span, TerminalLiteral,
    TerminalString, TypeBound, TypeParameter, Visibility,
};

#[derive(Clone, Debug)]
pub struct Grammar {
    // a unique prefix that can be appended to identifiers to ensure
    // that they do not conflict with any action strings
    pub prefix: String,

    // algorithm user requested for this parser
    pub algorithm: Algorithm,

    // true if the grammar mentions the `!` terminal anywhere
    pub uses_error_recovery: bool,

    // these are the nonterminals that were declared to be public; the
    // key is the user's name for the symbol, the value is the
    // artificial symbol we introduce, which will always have a single
    // production like `Foo' = Foo`.
    pub start_nonterminals: Map<NonterminalString, NonterminalString>,

    // the "use foo;" statements that the user declared
    pub uses: Vec<String>,

    // type parameters declared on the grammar, like `grammar<T>;`
    pub type_parameters: Vec<TypeParameter>,

    // actual parameters declared on the grammar, like the `x: u32` in `grammar(x: u32);`
    pub parameters: Vec<Parameter>,

    // where clauses declared on the grammar, like `grammar<T> where T: Sized`
    pub where_clauses: Vec<WhereClause>,

    // optional tokenizer DFA; this is only needed if the user did not supply
    // an extern token declaration
    pub intern_token: Option<InternToken>,

    // the grammar proper:
    pub action_fn_defns: Vec<ActionFnDefn>,
    pub terminals: TerminalSet,
    pub nonterminals: Map<NonterminalString, NonterminalData>,
    pub token_span: Span,
    pub conversions: Map<TerminalString, Pattern<TypeRepr>>,
    pub types: Types,
    pub module_attributes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum WhereClause {
    // forall<'a> WC
    Forall {
        binder: Vec<TypeParameter>,
        clause: Box<WhereClause>,
    },

    // `T: Foo`
    Bound {
        subject: TypeRepr,
        bound: TypeBound<TypeRepr>,
    },
}

/// For each terminal, we map it to a small integer from 0 to N.
/// This struct contains the mappings to go back and forth.
#[derive(Clone, Debug)]
pub struct TerminalSet {
    pub all: Vec<TerminalString>,
    pub bits: Map<TerminalString, usize>,
}

#[derive(Clone, Debug)]
pub struct NonterminalData {
    pub name: NonterminalString,
    pub visibility: Visibility,
    pub span: Span,
    pub annotations: Vec<Annotation>,
    pub productions: Vec<Production>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Algorithm {
    pub lalr: bool,
    pub codegen: LrCodeGeneration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LrCodeGeneration {
    TableDriven,
    RecursiveAscent,
    TestAll,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parameter {
    pub name: Atom,
    pub ty: TypeRepr,
}

#[derive(Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Production {
    // this overlaps with the key in the hashmap, obviously, but it's
    // handy to have it
    pub nonterminal: NonterminalString,
    pub symbols: Vec<Symbol>,
    pub action: ActionFn,
    pub span: Span,
}

#[derive(Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum Symbol {
    Nonterminal(NonterminalString),
    Terminal(TerminalString),
}

#[derive(Clone, PartialEq, Eq)]
pub struct ActionFnDefn {
    pub fallible: bool,
    pub ret_type: TypeRepr,
    pub kind: ActionFnDefnKind,
}

#[derive(Clone, PartialEq, Eq)]
pub enum ActionFnDefnKind {
    User(UserActionFnDefn),
    Inline(InlineActionFnDefn),
    Lookaround(LookaroundActionFnDefn),
}

/// An action fn written by a user.
#[derive(Clone, PartialEq, Eq)]
pub struct UserActionFnDefn {
    pub arg_patterns: Vec<Atom>,
    pub arg_types: Vec<TypeRepr>,
    pub code: String,
}

/// An action fn generated by the inlining pass.  If we were
/// inlining `A = B C D` (with action 44) into `X = Y A Z` (with
/// action 22), this would look something like:
///
/// ```
/// fn __action66(__0: Y, __1: B, __2: C, __3: D, __4: Z) {
///     __action22(__0, __action44(__1, __2, __3), __4)
/// }
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct InlineActionFnDefn {
    /// in the example above, this would be `action22`
    pub action: ActionFn,

    /// in the example above, this would be `Y, {action44: B, C, D}, Z`
    pub symbols: Vec<InlinedSymbol>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LookaroundActionFnDefn {
    Lookahead,
    Lookbehind,
}

#[derive(Clone, PartialEq, Eq)]
pub enum InlinedSymbol {
    Original(Symbol),
    Inlined(ActionFn, Vec<Symbol>),
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum TypeRepr {
    Tuple(Vec<TypeRepr>),
    Nominal(NominalTypeRepr),
    Associated {
        type_parameter: Atom,
        id: Atom,
    },
    Lifetime(Lifetime),
    Ref {
        lifetime: Option<Lifetime>,
        mutable: bool,
        referent: Box<TypeRepr>,
    },
}

impl TypeRepr {
    pub fn from_parameter(tp: &TypeParameter) -> Self {
        match tp {
            TypeParameter::Lifetime(l) => TypeRepr::Lifetime(l.clone()),
            TypeParameter::Id(name) => TypeRepr::Nominal(NominalTypeRepr {
                path: Path::from_id(name.clone()),
                types: vec![],
            }),
        }
    }

    pub fn is_unit(&self) -> bool {
        match *self {
            TypeRepr::Tuple(ref v) => v.is_empty(),
            _ => false,
        }
    }

    pub fn usize() -> TypeRepr {
        TypeRepr::Nominal(NominalTypeRepr {
            path: Path::usize(),
            types: vec![],
        })
    }

    pub fn str() -> TypeRepr {
        TypeRepr::Nominal(NominalTypeRepr {
            path: Path::str(),
            types: vec![],
        })
    }

    pub fn bottom_up(&self, op: &mut impl FnMut(TypeRepr) -> TypeRepr) -> Self {
        let result = match self {
            TypeRepr::Tuple(types) => {
                TypeRepr::Tuple(types.iter().map(|t| t.bottom_up(op)).collect())
            }
            TypeRepr::Nominal(NominalTypeRepr { path, types }) => {
                TypeRepr::Nominal(NominalTypeRepr {
                    path: path.clone(),
                    types: types.iter().map(|t| t.bottom_up(op)).collect(),
                })
            }
            TypeRepr::Associated { type_parameter, id } => TypeRepr::Associated {
                type_parameter: type_parameter.clone(),
                id: id.clone(),
            },
            TypeRepr::Lifetime(l) => TypeRepr::Lifetime(l.clone()),
            TypeRepr::Ref {
                lifetime,
                mutable,
                referent,
            } => TypeRepr::Ref {
                lifetime: lifetime.clone(),
                mutable: *mutable,
                referent: Box::new(referent.bottom_up(op)),
            },
        };
        op(result)
    }

    /// Finds anonymous lifetimes (e.g., `&u32` or `Foo<'_>`) and
    /// instantiates them with a name like `__1`. Also computes
    /// obvious outlives relationships that are needed (e.g., `&'a T`
    /// requires `T: 'a`). The parameters `type_parameters` and
    /// `where_clauses` should contain -- on entry -- the
    /// type-parameters and where-clauses that currently exist on the
    /// grammar. On exit, they will have been modified to include the
    /// new type parameters and any implied where clauses.
    pub fn name_anonymous_lifetimes_and_compute_implied_outlives(
        &self,
        prefix: &str,
        type_parameters: &mut Vec<TypeParameter>,
        where_clauses: &mut Vec<WhereClause>,
    ) -> Self {
        let fresh_lifetime_name = |type_parameters: &mut Vec<TypeParameter>| {
            // Make a name like `__1`:
            let len = type_parameters.len();
            let name = Lifetime(Atom::from(format!("'{}{}", prefix, len)));
            type_parameters.push(TypeParameter::Lifetime(name.clone()));
            name
        };

        self.bottom_up(&mut |t| match t {
            TypeRepr::Tuple { .. } | TypeRepr::Nominal { .. } | TypeRepr::Associated { .. } => t,

            TypeRepr::Lifetime(l) => {
                if l.is_anonymous() {
                    TypeRepr::Lifetime(fresh_lifetime_name(type_parameters))
                } else {
                    TypeRepr::Lifetime(l)
                }
            }

            TypeRepr::Ref {
                mut lifetime,
                mutable,
                referent,
            } => {
                if lifetime.is_none() {
                    lifetime = Some(fresh_lifetime_name(type_parameters));
                }

                // If we have `&'a T`, then we have to compute each
                // free variable `X` in `T` and ensure that `X: 'a`:
                let l = lifetime.clone().unwrap();
                for tp in referent.free_variables(type_parameters) {
                    let wc = WhereClause::Bound {
                        subject: TypeRepr::from_parameter(&tp),
                        bound: TypeBound::Lifetime(l.clone()),
                    };
                    if !where_clauses.contains(&wc) {
                        where_clauses.push(wc);
                    }
                }

                TypeRepr::Ref {
                    lifetime,
                    mutable,
                    referent,
                }
            }
        })
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct NominalTypeRepr {
    pub path: Path,
    pub types: Vec<TypeRepr>,
}

#[derive(Clone, Debug)]
pub struct Types {
    terminal_token_type: TypeRepr,
    terminal_loc_type: Option<TypeRepr>,
    error_type: Option<TypeRepr>,
    terminal_types: Map<TerminalString, TypeRepr>,
    nonterminal_types: Map<NonterminalString, TypeRepr>,
    parse_error_type: TypeRepr,
    error_recovery_type: TypeRepr,
}

impl Types {
    pub fn new(
        prefix: &str,
        terminal_loc_type: Option<TypeRepr>,
        error_type: Option<TypeRepr>,
        terminal_token_type: TypeRepr,
    ) -> Types {
        let mut types = Types {
            terminal_loc_type,
            error_type,
            terminal_token_type,
            terminal_types: map(),
            nonterminal_types: map(),
            // the following two will be overwritten later
            parse_error_type: TypeRepr::Tuple(vec![]),
            error_recovery_type: TypeRepr::Tuple(vec![]),
        };

        let args = vec![
            types.terminal_loc_type().clone(),
            types.terminal_token_type().clone(),
            types.error_type(),
        ];
        types.parse_error_type = TypeRepr::Nominal(NominalTypeRepr {
            path: Path {
                absolute: false,
                ids: vec![
                    Atom::from(format!("{}lalrpop_util", prefix)),
                    Atom::from("ParseError"),
                ],
            },
            types: args.clone(),
        });
        types.error_recovery_type = TypeRepr::Nominal(NominalTypeRepr {
            path: Path {
                absolute: false,
                ids: vec![
                    Atom::from(format!("{}lalrpop_util", prefix)),
                    Atom::from("ErrorRecovery"),
                ],
            },
            types: args,
        });
        types
            .terminal_types
            .insert(TerminalString::Error, types.error_recovery_type.clone());
        types
    }

    pub fn add_type(&mut self, nt_id: NonterminalString, ty: TypeRepr) {
        assert!(self.nonterminal_types.insert(nt_id, ty).is_none());
    }

    pub fn add_term_type(&mut self, term: TerminalString, ty: TypeRepr) {
        assert!(self.terminal_types.insert(term, ty).is_none());
    }

    pub fn terminal_token_type(&self) -> &TypeRepr {
        &self.terminal_token_type
    }

    pub fn opt_terminal_loc_type(&self) -> Option<&TypeRepr> {
        self.terminal_loc_type.as_ref()
    }

    pub fn terminal_loc_type(&self) -> TypeRepr {
        self.terminal_loc_type
            .clone()
            .unwrap_or_else(|| TypeRepr::Tuple(vec![]))
    }

    pub fn error_type(&self) -> TypeRepr {
        self.error_type.clone().unwrap_or_else(|| TypeRepr::Ref {
            lifetime: Some(Lifetime::statik()),
            mutable: false,
            referent: Box::new(TypeRepr::str()),
        })
    }

    pub fn terminal_type(&self, id: &TerminalString) -> &TypeRepr {
        self.terminal_types
            .get(&id)
            .unwrap_or(&self.terminal_token_type)
    }

    pub fn terminal_types(&self) -> Vec<TypeRepr> {
        self.terminal_types.values().cloned().collect()
    }

    pub fn lookup_nonterminal_type(&self, id: &NonterminalString) -> Option<&TypeRepr> {
        self.nonterminal_types.get(&id)
    }

    pub fn nonterminal_type(&self, id: &NonterminalString) -> &TypeRepr {
        &self.nonterminal_types[&id]
    }

    pub fn nonterminal_types(&self) -> Vec<TypeRepr> {
        self.nonterminal_types.values().cloned().collect()
    }

    pub fn parse_error_type(&self) -> &TypeRepr {
        &self.parse_error_type
    }

    pub fn error_recovery_type(&self) -> &TypeRepr {
        &self.error_recovery_type
    }

    /// Returns a type `(L, T, L)` where L is the location type and T
    /// is the token type.
    pub fn triple_type(&self) -> TypeRepr {
        self.spanned_type(self.terminal_token_type().clone())
    }

    /// Returns a type `(L, T, L)` where L is the location type and T
    /// is the argument.
    pub fn spanned_type(&self, ty: TypeRepr) -> TypeRepr {
        let location_type = self.terminal_loc_type();
        TypeRepr::Tuple(vec![location_type.clone(), ty, location_type])
    }
}

impl Display for WhereClause {
    fn fmt(&self, fmt: &mut Formatter) -> Result<(), Error> {
        match self {
            WhereClause::Forall { binder, clause } => {
                write!(fmt, "for<{}> {}", Sep(", ", binder), clause)
            }

            WhereClause::Bound { subject, bound } => write!(fmt, "{}: {}", subject, bound),
        }
    }
}

impl Display for Parameter {
    fn fmt(&self, fmt: &mut Formatter) -> Result<(), Error> {
        write!(fmt, "{}: {}", self.name, self.ty)
    }
}

impl Display for TypeRepr {
    fn fmt(&self, fmt: &mut Formatter) -> Result<(), Error> {
        match *self {
            TypeRepr::Tuple(ref types) => write!(fmt, "({})", Sep(", ", types)),
            TypeRepr::Nominal(ref data) => write!(fmt, "{}", data),
            TypeRepr::Associated {
                ref type_parameter,
                ref id,
            } => write!(fmt, "{}::{}", type_parameter, id),
            TypeRepr::Lifetime(ref id) => write!(fmt, "{}", id),
            TypeRepr::Ref {
                lifetime: None,
                mutable: false,
                ref referent,
            } => write!(fmt, "&{}", referent),
            TypeRepr::Ref {
                lifetime: Some(ref l),
                mutable: false,
                ref referent,
            } => write!(fmt, "&{} {}", l, referent),
            TypeRepr::Ref {
                lifetime: None,
                mutable: true,
                ref referent,
            } => write!(fmt, "&mut {}", referent),
            TypeRepr::Ref {
                lifetime: Some(ref l),
                mutable: true,
                ref referent,
            } => write!(fmt, "&{} mut {}", l, referent),
        }
    }
}

impl Debug for TypeRepr {
    fn fmt(&self, fmt: &mut Formatter) -> Result<(), Error> {
        Display::fmt(self, fmt)
    }
}

impl Display for NominalTypeRepr {
    fn fmt(&self, fmt: &mut Formatter) -> Result<(), Error> {
        if self.types.is_empty() {
            write!(fmt, "{}", self.path)
        } else {
            write!(fmt, "{}<{}>", self.path, Sep(", ", &self.types))
        }
    }
}

impl Debug for NominalTypeRepr {
    fn fmt(&self, fmt: &mut Formatter) -> Result<(), Error> {
        Display::fmt(self, fmt)
    }
}

#[derive(Copy, Clone, Debug, Hash, PartialOrd, Ord, PartialEq, Eq)]
pub struct ActionFn(u32);

impl ActionFn {
    pub fn new(x: usize) -> ActionFn {
        ActionFn(x as u32)
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl Symbol {
    pub fn is_terminal(&self) -> bool {
        match *self {
            Symbol::Terminal(..) => true,
            Symbol::Nonterminal(..) => false,
        }
    }

    pub fn ty<'ty>(&self, t: &'ty Types) -> &'ty TypeRepr {
        match *self {
            Symbol::Terminal(ref id) => t.terminal_type(id),
            Symbol::Nonterminal(ref id) => t.nonterminal_type(id),
        }
    }
}

impl Display for Symbol {
    fn fmt(&self, fmt: &mut Formatter) -> Result<(), Error> {
        match *self {
            Symbol::Nonterminal(ref id) => write!(fmt, "{}", id.clone()),
            Symbol::Terminal(ref id) => write!(fmt, "{}", id.clone()),
        }
    }
}

impl Debug for Symbol {
    fn fmt(&self, fmt: &mut Formatter) -> Result<(), Error> {
        Display::fmt(self, fmt)
    }
}

impl Into<Box<Content>> for Symbol {
    fn into(self) -> Box<Content> {
        match self {
            Symbol::Nonterminal(nt) => nt.into(),
            Symbol::Terminal(term) => term.into(),
        }
    }
}

impl Debug for Production {
    fn fmt(&self, fmt: &mut Formatter) -> Result<(), Error> {
        write!(
            fmt,
            "{} = {} => {:?};",
            self.nonterminal,
            Sep(", ", &self.symbols),
            self.action,
        )
    }
}

impl Debug for ActionFnDefn {
    fn fmt(&self, fmt: &mut Formatter) -> Result<(), Error> {
        write!(fmt, "{}", self.to_fn_string("_"))
    }
}

impl ActionFnDefn {
    fn to_fn_string(&self, name: &str) -> String {
        match self.kind {
            ActionFnDefnKind::User(ref data) => data.to_fn_string(self, name),
            ActionFnDefnKind::Inline(ref data) => data.to_fn_string(name),
            ActionFnDefnKind::Lookaround(ref data) => format!("{:?}", data),
        }
    }
}

impl UserActionFnDefn {
    fn to_fn_string(&self, defn: &ActionFnDefn, name: &str) -> String {
        let arg_strings: Vec<String> = self
            .arg_patterns
            .iter()
            .zip(self.arg_types.iter())
            .map(|(p, t)| format!("{}: {}", p, t))
            .collect();

        format!(
            "fn {}({}) -> {} {{ {} }}",
            name,
            Sep(", ", &arg_strings),
            defn.ret_type,
            self.code,
        )
    }
}

impl InlineActionFnDefn {
    fn to_fn_string(&self, name: &str) -> String {
        let arg_strings: Vec<String> = self
            .symbols
            .iter()
            .map(|inline_sym| match *inline_sym {
                InlinedSymbol::Original(ref s) => format!("{}", s),
                InlinedSymbol::Inlined(a, ref s) => format!("{:?}({})", a, Sep(", ", s)),
            })
            .collect();

        format!(
            "fn {}(..) {{ {:?}({}) }}",
            name,
            self.action,
            Sep(", ", &arg_strings),
        )
    }
}

impl Grammar {
    pub fn pattern(&self, t: &TerminalString) -> &Pattern<TypeRepr> {
        &self.conversions[t]
    }

    pub fn productions_for(&self, nonterminal: &NonterminalString) -> &[Production] {
        match self.nonterminals.get(nonterminal) {
            Some(v) => &v.productions[..],
            None => &[], // this...probably shouldn't happen actually?
        }
    }

    pub fn user_parameter_refs(&self) -> String {
        let mut result = String::new();
        for parameter in &self.parameters {
            result.push_str(&format!("{}, ", parameter.name));
        }
        result
    }

    pub fn action_is_fallible(&self, f: ActionFn) -> bool {
        self.action_fn_defns[f.index()].fallible
    }

    pub fn non_lifetime_type_parameters(&self) -> Vec<&TypeParameter> {
        self.type_parameters
            .iter()
            .filter(|&tp| match *tp {
                TypeParameter::Lifetime(_) => false,
                TypeParameter::Id(_) => true,
            })
            .collect()
    }
}

impl Default for Algorithm {
    fn default() -> Self {
        Algorithm {
            lalr: false,
            codegen: LrCodeGeneration::TableDriven,
        }
    }
}
