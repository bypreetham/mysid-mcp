//! The Firebase JavaScript SDK (modular API) -> FlutterFire (`firebase_core`, `firebase_auth`, `cloud_firestore`).
//!
//! `getDoc(doc(db, 'admins', uid))` becomes `db.collection('admins').doc(uid).get()`, `where('a', '==', x)` becomes
//! `.where('a', isEqualTo: x)`, `signInWithEmailAndPassword(auth, e, p)` becomes `auth.signInWithEmailAndPassword(email: e, password: p)`.

use super::ast::*;
use super::cx::*;
use super::expr::{str_lit, unparen};
use super::types::*;

fn where_op(op: &str) -> Option<&'static str> {
    Some(match op {
        "==" | "===" => "isEqualTo",
        "!=" | "!==" => "isNotEqualTo",
        "<" => "isLessThan",
        "<=" => "isLessThanOrEqualTo",
        ">" => "isGreaterThan",
        ">=" => "isGreaterThanOrEqualTo",
        "array-contains" => "arrayContains",
        "array-contains-any" => "arrayContainsAny",
        "in" => "whereIn",
        "not-in" => "whereNotIn",
        _ => return None,
    })
}

impl<'a> Cx<'a> {
    /// A value handed to the database: a model becomes its map.
    fn db_data(&mut self, e: &Expr) -> String {
        let code = unparen(&self.expr(e, None));
        match self.expr_type(e).and_then(|t| self.model_name(&t)) {
            Some(_) => format!("{}.toMap()", code),
            None => code,
        }
    }

    fn is_db_root(&self, e: &Expr) -> bool {
        match e {
            Expr::Ident(n) => matches!(n.as_str(), "db" | "firestore" | "database") || self.firebase.get(n).map_or(false, |o| o == "getFirestore"),
            Expr::Call(c, _, _) => matches!(&**c, Expr::Ident(n) if self.firebase.get(n).map_or(false, |o| o == "getFirestore")),
            _ => false,
        }
    }

    /// `where('a', '>=', x)` / `orderBy('a', 'desc')` / `limit(n)` inside `query(...)`, as a method call on the query.
    fn query_constraint(&mut self, e: &Expr) -> Option<String> {
        let Expr::Call(callee, args, _) = e else { return None };
        let Expr::Ident(n) = &**callee else { return None };
        let orig = self.firebase.get(n)?.clone();
        let arg = |cx: &mut Self, i: usize| args.get(i).map(|a| unparen(&cx.expr(a, None))).unwrap_or_default();
        match orig.as_str() {
            "where" => {
                let op = match args.get(1) {
                    Some(Expr::Str(s)) => where_op(s)?,
                    _ => return None,
                };
                Some(format!(".where({}, {}: {})", arg(self, 0), op, arg(self, 2)))
            }
            "orderBy" => {
                let desc = matches!(args.get(1), Some(Expr::Str(s)) if s == "desc");
                Some(format!(".orderBy({}{})", arg(self, 0), if desc { ", descending: true" } else { "" }))
            }
            "limit" => Some(format!(".limit({})", arg(self, 0))),
            "startAfter" => Some(format!(".startAfter([{}])", arg(self, 0))),
            "startAt" => Some(format!(".startAt([{}])", arg(self, 0))),
            "endBefore" => Some(format!(".endBefore([{}])", arg(self, 0))),
            "endAt" => Some(format!(".endAt([{}])", arg(self, 0))),
            _ => None,
        }
    }

    /// The FlutterFire spelling of a call of the Firebase JS SDK, or None when the call is not known.
    pub fn firebase_call(&mut self, orig: &str, args: &[Expr]) -> Option<String> {
        let a = |cx: &mut Self, i: usize| args.get(i).map(|x| unparen(&cx.expr(x, None))).unwrap_or_default();
        Some(match orig {
            // ---- Firestore
            "getFirestore" => "FirebaseFirestore.instance".to_string(),
            "collection" | "doc" => {
                let first = args.first()?;
                let mut code = a(self, 0);
                if self.is_db_root(first) {
                    // root, then collection / doc / collection / ...
                    for (i, seg) in args.iter().skip(1).enumerate() {
                        let s = unparen(&self.expr(seg, None));
                        code = format!("{}.{}({})", code, if i % 2 == 0 { "collection" } else { "doc" }, s);
                    }
                    if orig == "doc" && args.len() == 2 {
                        // doc(db, 'a') has no id: not valid, keep the shape
                    }
                } else if orig == "doc" {
                    code = match args.get(1) {
                        Some(id) => format!("{}.doc({})", code, unparen(&self.expr(id, None))),
                        None => format!("{}.doc()", code),
                    };
                } else {
                    for seg in args.iter().skip(1) {
                        code = format!("{}.collection({})", code, unparen(&self.expr(seg, None)));
                    }
                }
                code
            }
            "getDoc" | "getDocs" => format!("{}.get()", a(self, 0)),
            "setDoc" => {
                let data = self.db_data(args.get(1)?);
                let merge = matches!(args.get(2), Some(Expr::Object(p)) if p.iter().any(|x| matches!(x, Prop::Kv(PropKey::Name(k), Expr::Bool(true)) if k == "merge")));
                format!("{}.set({}{})", a(self, 0), data, if merge { ", SetOptions(merge: true)" } else { "" })
            }
            "updateDoc" => {
                let data = self.db_data(args.get(1)?);
                format!("{}.update({})", a(self, 0), data)
            }
            "deleteDoc" => format!("{}.delete()", a(self, 0)),
            "addDoc" => {
                let data = self.db_data(args.get(1)?);
                format!("{}.add({})", a(self, 0), data)
            }
            "query" => {
                let mut code = a(self, 0);
                for c in args.iter().skip(1) {
                    match self.query_constraint(c) {
                        Some(s) => code.push_str(&s),
                        None => self.note("a Firestore query constraint is not one of where / orderBy / limit / startAfter: left out"),
                    }
                }
                code
            }
            "serverTimestamp" => "FieldValue.serverTimestamp()".to_string(),
            "arrayUnion" | "arrayRemove" => {
                let items: Vec<String> = args.iter().map(|x| unparen(&self.expr(x, None))).collect();
                format!("FieldValue.{}([{}])", orig, items.join(", "))
            }
            "increment" => format!("FieldValue.increment({})", a(self, 0)),
            "deleteField" => "FieldValue.delete()".to_string(),
            "runTransaction" => format!("{}.runTransaction({})", a(self, 0), a(self, 1)),
            "writeBatch" => format!("{}.batch()", a(self, 0)),
            "onSnapshot" => format!("jsUnsub({}.snapshots().listen({}))", a(self, 0), a(self, 1)),
            "enableIndexedDbPersistence" | "enableMultiTabIndexedDbPersistence" => {
                self.note("Firestore offline persistence is on by default in the mobile SDK: enableIndexedDbPersistence is a no-op");
                "Future<void>.value()".to_string()
            }
            // ---- Auth
            "getAuth" => "FirebaseAuth.instance".to_string(),
            "signInWithEmailAndPassword" | "createUserWithEmailAndPassword" => format!("{}.{}(email: {}, password: {})", a(self, 0), orig, a(self, 1), a(self, 2)),
            "signOut" => format!("{}.signOut()", a(self, 0)),
            "sendEmailVerification" => format!("{}.sendEmailVerification()", a(self, 0)),
            "sendPasswordResetEmail" => format!("{}.sendPasswordResetEmail(email: {})", a(self, 0), a(self, 1)),
            "onAuthStateChanged" => format!("jsUnsub({}.authStateChanges().listen({}))", a(self, 0), a(self, 1)),
            "signInWithPopup" | "signInWithRedirect" => format!("{}.signInWithProvider({})", a(self, 0), a(self, 1)),
            "signInWithPhoneNumber" => {
                self.note("phone sign-in: the web flow (RecaptchaVerifier + signInWithPhoneNumber) is not available on Android/iOS; use FirebaseAuth.verifyPhoneNumber");
                format!("{}.signInWithPhoneNumber({})", a(self, 0), a(self, 1))
            }
            "updateProfile" => match args.get(1) {
                Some(Expr::Object(props)) => {
                    let name = props.iter().find_map(|p| match p {
                        Prop::Kv(PropKey::Name(k), v) if k == "displayName" => Some(unparen(&self.expr(v, None))),
                        _ => None,
                    });
                    format!("{}.updateDisplayName({})", a(self, 0), name.unwrap_or_else(|| "null".to_string()))
                }
                _ => return None,
            },
            "setPersistence" => {
                self.note("auth persistence: the mobile SDK keeps the user signed in by itself, setPersistence is a no-op");
                "Future<void>.value()".to_string()
            }
            // ---- App
            "initializeApp" => {
                self.note("initializeApp: Firebase.initializeApp(options: DefaultFirebaseOptions.currentPlatform) runs in main(); pass the keys with --dart-define-from-file=.env");
                if let Some(Expr::Ident(cfg)) = args.first() {
                    self.firebase_config_of = Some(cfg.clone());
                }
                "null".to_string()
            }
            "getApp" | "getApps" => "Firebase.app()".to_string(),
            _ => return None,
        })
    }

    /// The `FirebaseOptions(...)` named arguments from a config object (`apiKey: import.meta.env.X, ...`).
    pub fn firebase_options_from(&mut self, config: &Expr) -> Option<String> {
        let Expr::Object(props) = config else { return None };
        let mut parts = Vec::new();
        for p in props {
            if let Prop::Kv(PropKey::Name(k), v) = p {
                if matches!(k.as_str(), "apiKey" | "authDomain" | "projectId" | "storageBucket" | "messagingSenderId" | "appId" | "measurementId" | "databaseURL") {
                    let code = unparen(&self.expr(v, None));
                    let key = if k == "databaseURL" { "databaseURL".to_string() } else { k.clone() };
                    parts.push(format!("      {}: {},", safe_ident(&key), code));
                }
            }
        }
        if parts.is_empty() { None } else { Some(parts.join("\n")) }
    }
}

#[allow(dead_code)]
fn unused(_: &str) -> String {
    str_lit("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::convert::parse::parse_module;

    fn convert(src: &str) -> String {
        let m = parse_module(&format!("import {{ getDoc, doc, setDoc, query, where, orderBy, collection, getDocs, runTransaction, serverTimestamp, arrayUnion, updateDoc }} from 'firebase/firestore';\nimport {{ signInWithEmailAndPassword, onAuthStateChanged, signOut }} from 'firebase/auth';\nasync function f() {{ {} }}", src));
        let mut info = ProjectInfo::default();
        info.models = Default::default();
        let mut cx = Cx::new(&info, "t.ts");
        cx.firebase_used = true;
        for it in &m.items {
            if let Item::Import(i) = it {
                for (o, l) in &i.named {
                    cx.firebase.insert(l.clone(), o.clone());
                }
            }
        }
        let Some(Item::Func { func, .. }) = m.items.iter().find(|i| matches!(i, Item::Func { .. })) else { panic!() };
        let FnBody::Block(b) = &func.body else { panic!() };
        cx.block(b, 0)
    }

    #[test]
    fn firestore_calls_become_flutterfire() {
        let s = convert("const snap = await getDoc(doc(db, 'admins', uid)); if (snap.exists()) { return snap.data(); }");
        assert!(s.contains("db.collection('admins').doc(uid).get()"), "{}", s);
        assert!(s.contains("snap.exists"), "{}", s);
        let q = convert("const q = query(collection(db, 'logs'), where('at', '>=', start), where('tag', 'array-contains', t), orderBy('at', 'desc')); const r = await getDocs(q);");
        assert!(q.contains("db.collection('logs').where('at', isGreaterThanOrEqualTo: start).where('tag', arrayContains: t).orderBy('at', descending: true)"), "{}", q);
        assert!(q.contains("q.get()"), "{}", q);
        let w = convert("await setDoc(doc(db, 'a', id), { n: 1, at: serverTimestamp() }, { merge: true }); await updateDoc(ref, { xs: arrayUnion(1, 2) });");
        assert!(w.contains("SetOptions(merge: true)") && w.contains("FieldValue.serverTimestamp()") && w.contains("FieldValue.arrayUnion([1, 2])"), "{}", w);
    }

    #[test]
    fn auth_calls_become_flutterfire() {
        let s = convert("const c = await signInWithEmailAndPassword(auth, email, password); const off = onAuthStateChanged(auth, (u) => { go(u); }); await signOut(auth);");
        assert!(s.contains("auth.signInWithEmailAndPassword(email: email, password: password)"), "{}", s);
        assert!(s.contains("jsUnsub(auth.authStateChanges().listen("), "{}", s);
        assert!(s.contains("auth.signOut()"), "{}", s);
    }
}
