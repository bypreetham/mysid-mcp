use super::{TypeScopes, gather_multiline_signature, LanguageParser, MethodSignature, TypeInfo};
use regex::Regex;

pub struct CSharpParser;

const MODS: &str = r"(?:\[[^\]]*\]\s*)*(?:(?:public|private|protected|internal|abstract|static|virtual|override|async|sealed|extern|new|unsafe|partial|readonly|const|required|file|ref)\s+)*";
const NOT_RETURN_TYPES: &[&str] = &["return", "new", "await", "throw", "yield", "else", "using", "in", "out", "case", "goto", "var", "lock"];
const NOT_NAMES: &[&str] = &["if", "while", "for", "foreach", "switch", "catch", "lock", "using", "fixed", "return", "nameof", "typeof", "sizeof"];

impl LanguageParser for CSharpParser {
    fn language_name(&self) -> &'static str {
        "C#"
    }

    fn extensions(&self) -> &[&'static str] {
        &["cs"]
    }

    fn parse(&self, lines: &[&str]) -> (Vec<TypeInfo>, Vec<MethodSignature>) {
        let mut types = Vec::new();
        let mut methods = Vec::new();

        let type_re = Regex::new(&format!(
            r"^{}(?:record\s+)?(class|interface|struct|record|enum)\s+([A-Za-z0-9_]+)",
            MODS
        ))
        .unwrap();
        // `public static Task<int> Name<T>(`, `void Name(` (interface members have no modifiers), `void IFoo.Name(`.
        let method_re = Regex::new(&format!(
            r"^{}([A-Za-z_][A-Za-z0-9_<>,.?\[\]\s]*?)\s+(?:[A-Za-z0-9_<>]+\.)?([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>(]*>)?\s*\(",
            MODS
        ))
        .unwrap();
        // `public Name(` / `~Name(` of the open type.
        let ctor_re = Regex::new(&format!(r"^{}(~?)([A-Za-z_][A-Za-z0-9_]*)\s*\(", MODS)).unwrap();
        // `public int Count { get; set; }`, `public string Name => _name;`
        let prop_re = Regex::new(&format!(
            r"^{}([A-Za-z_][A-Za-z0-9_<>,.?\[\]\s]*?)\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:\{{\s*(?:get|set|init)\b|=>)",
            MODS
        ))
        .unwrap();
        let prop_header_re = Regex::new(&format!(r"^{}([A-Za-z_][A-Za-z0-9_<>,.?\[\]\s]*?)\s+([A-Za-z_][A-Za-z0-9_]*)\s*$", MODS)).unwrap();

        let mut scopes = TypeScopes::new();

        for (idx, &line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') || trimmed.starts_with('#') {
                continue;
            }

            let member_level = scopes.at_member_level();
            if let Some(caps) = type_re.captures(trimmed) {
                let kind = caps.get(1).map_or("class", |m| m.as_str());
                let name = caps.get(2).map_or("", |m| m.as_str());
                types.push(TypeInfo {
                    name: name.to_string(),
                    kind: kind.to_string(),
                    line: idx + 1,
                });
                scopes.enter(name);
            } else if member_level {
                let current_type = scopes.current();
                let mut found: Option<(String, &str, Option<String>)> = None;
                let ctor = ctor_re.captures(trimmed).filter(|c| current_type.as_deref() == Some(&c[2]));
                if let Some(c) = ctor {
                    found = Some((format!("{}{}", &c[1], &c[2]), if c[1].is_empty() { "constructor" } else { "destructor" }, None));
                } else if let Some(c) = method_re.captures(trimmed) {
                    let ret = c[1].trim().to_string();
                    let first = ret.split_whitespace().next().unwrap_or("");
                    if !NOT_NAMES.contains(&&c[2]) && !NOT_RETURN_TYPES.contains(&first) {
                        found = Some((c[2].to_string(), "method", Some(ret)));
                    }
                } else if let Some(c) = prop_re.captures(trimmed) {
                    found = Some((c[2].to_string(), "property", Some(c[1].trim().to_string())));
                } else if !trimmed.contains('(') && !trimmed.contains('=') && !trimmed.ends_with(';') {
                    // `public int Count` with the `{ get; set; }` on the following lines.
                    let next_is_open = lines.get(idx + 1).map_or(false, |n| n.trim() == "{");
                    let accessor = lines.get(idx + 2).map_or(false, |n| {
                        let t = n.trim_start();
                        t.starts_with("get") || t.starts_with("set") || t.starts_with("init") || t.starts_with("private set") || t.starts_with("protected set")
                    });
                    if next_is_open && accessor {
                        if let Some(c) = prop_header_re.captures(trimmed) {
                            let first = c[1].split_whitespace().next().unwrap_or("");
                            if !NOT_RETURN_TYPES.contains(&first) && !matches!(&c[2], "get" | "set") {
                                found = Some((c[2].to_string(), "property", Some(c[1].trim().to_string())));
                            }
                        }
                    }
                }
                if let Some((name, kind, return_type)) = found {
                    let (sig, params, end_line) = if kind == "property" {
                        (trimmed.trim_end_matches('{').trim().to_string(), String::new(), idx + 1)
                    } else {
                        gather_multiline_signature(lines, idx, '{')
                    };
                    let sig = match sig.find("=>") {
                        Some(p) => sig[..p].trim().to_string(),
                        None => sig,
                    };
                    methods.push(MethodSignature {
                        name,
                        kind: kind.to_string(),
                        signature: sig,
                        params,
                        return_type,
                        start_line: idx + 1,
                        end_line,
                        parent: current_type,
                    });
                }
            }
            scopes.end_line(line, lines.get(idx + 1).copied());
        }

        (types, methods)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn members_constructors_properties_and_interface_methods() {
        let src = r#"namespace App.Audio
{
    public interface IPlayer
    {
        void Play();
        Task<int> LoadAsync(string path);
    }

    public sealed class Engine : IPlayer, IDisposable
    {
        [DllImport("engine.dll")] private static extern bool EngineLoad(IntPtr e, string uri);
        private readonly int _h;
        public int Count { get; private set; }
        public string Name => _name;
        public double Gain
        {
            get { return _g; }
            set { _g = value; }
        }

        public Engine(int h)
            : this()
        {
            var x = Make(h);
            if (x != null) { Run(x); }
            return;
        }

        ~Engine() => Dispose();

        public async Task<int> LoadAsync(string path) => await Task.Run(() => 1);
        void IDisposable.Dispose() { Stop(); }
        public static T Get<T>(int i) where T : class
        {
            Action a = () => Foo(1);
            return Helper<T>(i);
        }
    }
}
"#;
        let lines: Vec<&str> = src.lines().collect();
        let (types, m) = CSharpParser.parse(&lines);
        assert!(types.iter().any(|t| t.name == "Engine" && t.kind == "class") && types.iter().any(|t| t.name == "IPlayer"));
        let find = |n: &str, k: &str| m.iter().find(|x| x.name == n && x.kind == k).unwrap_or_else(|| panic!("no {} {} in {:?}", k, n, m.iter().map(|x| (&x.name, &x.kind)).collect::<Vec<_>>()));
        assert_eq!(find("Play", "method").parent.as_deref(), Some("IPlayer"));
        assert_eq!(find("LoadAsync", "method").return_type.is_some(), true);
        assert_eq!(find("EngineLoad", "method").parent.as_deref(), Some("Engine"));
        assert_eq!(find("Count", "property").return_type.as_deref(), Some("int"));
        assert_eq!(find("Name", "property").signature, "public string Name");
        assert_eq!(find("Gain", "property").return_type.as_deref(), Some("double"));
        assert_eq!(find("Engine", "constructor").parent.as_deref(), Some("Engine"));
        assert_eq!(find("~Engine", "destructor").parent.as_deref(), Some("Engine"));
        assert_eq!(find("Dispose", "method").parent.as_deref(), Some("Engine"));
        assert_eq!(find("Get", "method").return_type.as_deref(), Some("T"));
        // Statements inside bodies are never members.
        for stmt in ["Make", "Run", "Foo", "Helper", "Task", "get", "set"] {
            assert!(m.iter().all(|x| x.name != stmt), "{} must not be a member", stmt);
        }
    }
}
