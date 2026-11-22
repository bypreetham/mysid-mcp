//! What the Dart emitter knows while it writes one file: the project's models and signatures, and the current scope.

use super::ast::*;
use super::types::*;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
pub struct FuncSig {
    /// (name, type, optional)
    pub params: Vec<(String, Option<Ty>, bool)>,
    pub ret: Option<Ty>,
}

/// Everything about the whole project that one file's output depends on.
#[derive(Default)]
pub struct ProjectInfo {
    pub models: Models,
    /// interfaces that are only the props of a component (written as constructor parameters, not as classes)
    pub props_only: HashSet<String>,
    pub funcs: HashMap<String, FuncSig>,
    /// component name -> its props
    pub comps: HashMap<String, Vec<DField>>,
    /// functions named `useXxx` that live in the project: they take the BuildContext first
    pub hooks: HashSet<String>,
    pub contexts: HashSet<String>,
    /// names imported from packages the converter has no Dart equivalent for: (name, module)
    pub externals: Vec<(String, String)>,
    /// project-level names that hold a widget-valued constant or lazy page
    pub lazy_pages: HashMap<String, String>,
}

#[derive(Default, Clone)]
pub struct Scope {
    /// `setX` -> (state field `x`, its type)
    pub setters: HashMap<String, (String, Option<Ty>)>,
    /// identifier -> replacement text (props read through `widget.` in a State class, and so on)
    pub renames: HashMap<String, String>,
    /// names bound to `useNavigate()`
    pub nav: HashSet<String>,
    /// handler parameters that stand for a DOM event
    pub events: Vec<String>,
    /// locals that hold a widget
    pub widgets: HashSet<String>,
    /// state fields (assignments to them inside handlers are wrapped in setState)
    pub state_fields: HashSet<String>,
    pub in_state: bool,
    /// `this.state.x` and `this.props.x` of a class component
    pub class_component: bool,
    /// methods of the component with the number of parameters they take (a handler that takes an event gets `null`)
    pub methods: HashMap<String, usize>,
    /// props that hold an icon (`icon: Icon` used as `<Icon/>`)
    pub icon_vars: HashSet<String>,
    /// inside a widget's `build`: a bare `return` returns an empty widget
    pub in_build: bool,
    /// props typed `ReactNode` (`Widget?` in Dart): shown with a fallback when null
    pub nullable_widgets: HashSet<String>,
    /// `...rest` of the props: its name and the prop fields it stands for (spread onto an element, they become attributes)
    pub rest: Option<(String, Vec<String>)>,
}

pub struct Cx<'a> {
    pub info: &'a ProjectInfo,
    pub notes: Vec<String>,
    pub scope: Scope,
    pub vars: Vec<HashMap<String, Ty>>,
    pub catch_params: Vec<String>,
    pub uses_tw: bool,
    pub file: String,
    pub tmp: usize,
    /// string constants (class lists) visible to the file, for folding `className={`${base} ...`}`
    pub consts: HashMap<String, String>,
    /// the `onSubmit` of the enclosing <form>, as a no-argument closure
    pub forms: Vec<Option<String>>,
    /// names imported from lucide-react
    pub lucide_imports: HashSet<String>,
    pub app_title: String,
    /// every name declared so far (state, getters, methods, parameters, locals), typed or not
    pub names: HashSet<String>,
    /// local name of a lucide icon -> its real name (`Phone as PhoneIcon`)
    pub lucide_alias: HashMap<String, String>,
    /// names imported from `firebase/*`: local name -> the SDK's name (`getDoc`, `where`, ...)
    pub firebase: HashMap<String, String>,
    pub firebase_used: bool,
    /// the name of the config object given to `initializeApp(config)`
    pub firebase_config_of: Option<String>,
    /// the FirebaseOptions parameters found in `initializeApp(config)`, as Dart named arguments
    pub firebase_options: Option<String>,
    /// `import { a as b }`: `b` is read as `a` (qualified with the import prefix)
    pub import_alias: HashMap<String, String>,
    /// names bound by an object `...rest`: Maps, turned back into a model where one is expected
    pub map_vars: HashSet<String>,
    /// the element type a callback's first parameter gets (`xs.map(x => ...)` with `xs: T[]`)
    pub closure_hint: Option<Ty>,
    /// the props type of the component being read when it is given on the variable (`React.FC<Props>`)
    pub props_hint: Option<Ty>,
}

impl ProjectInfo {
    /// The fields of an interface that serves as a component's props.
    pub fn comps_by_type(&self, name: &str) -> Vec<DField> {
        self.models.get(name).cloned().unwrap_or_default()
    }
}

impl<'a> Cx<'a> {
    pub fn new(info: &'a ProjectInfo, file: &str) -> Cx<'a> {
        Cx {
            info,
            notes: Vec::new(),
            scope: Scope::default(),
            vars: vec![HashMap::new()],
            catch_params: Vec::new(),
            uses_tw: false,
            file: file.to_string(),
            tmp: 0,
            consts: HashMap::new(),
            forms: Vec::new(),
            lucide_imports: HashSet::new(),
            app_title: "App".to_string(),
            names: HashSet::new(),
            lucide_alias: HashMap::new(),
            import_alias: HashMap::new(),
            firebase: HashMap::new(),
            firebase_used: false,
            firebase_config_of: None,
            firebase_options: None,
            closure_hint: None,
            map_vars: HashSet::new(),
            props_hint: None,
        }
    }

    pub fn note(&mut self, msg: impl Into<String>) {
        let m = format!("{}: {}", self.file, msg.into());
        if !self.notes.contains(&m) {
            self.notes.push(m);
        }
    }

    pub fn fresh(&mut self, base: &str) -> String {
        self.tmp += 1;
        format!("_{}{}", base, self.tmp)
    }

    pub fn dty(&self, t: &Ty) -> String {
        dart_type(t, &self.info.models)
    }

    pub fn push_scope(&mut self) {
        self.vars.push(HashMap::new());
    }

    pub fn pop_scope(&mut self) {
        self.vars.pop();
    }

    pub fn declare(&mut self, name: &str, ty: Option<Ty>) {
        self.names.insert(name.to_string());
        if let (Some(t), Some(top)) = (ty, self.vars.last_mut()) {
            top.insert(name.to_string(), t);
        }
    }

    pub fn type_of_name(&self, name: &str) -> Option<&Ty> {
        self.vars.iter().rev().find_map(|v| v.get(name))
    }

    pub fn model_name(&self, t: &Ty) -> Option<String> {
        model_of(t, &self.info.models)
    }
}

pub fn ind(n: usize) -> String {
    "  ".repeat(n)
}
