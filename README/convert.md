# mysid convert: a React / Tailwind project into a Flutter project

```
mysid convert <react-project> <flutter-project>              replace <flutter-project>/lib with the converted app
mysid convert <react-project> <flutter-project> --dry-run    convert and report, write nothing
mysid convert <react-project> <flutter-project> --analyze    also run `dart analyze lib` on the result
mysid convert <react-project> <flutter-project> --notes      list every construct that needed a stand-in
```

The Flutter project must already exist (`flutter create`): its `lib/` is replaced and the old one is kept in
`<flutter-project>/.mysid/lib-backup-<time>`. `test/widget_test.dart` of a new scaffold still names `MyApp`: delete or
rewrite it after converting.

## How it works

```
.ts / .tsx  --parse-->  syntax tree  --lower-->  Dart text        (convert/parse.rs, ast.rs)
class="..." --tailwind-->  Style     --widgets-->  Flutter widgets (convert/tailwind.rs, widgets.rs)
```

- **Parser** (`parse.rs`): a real recursive-descent reader for TypeScript and TSX (expressions, statements, types, JSX, classes). The
  regex parsers of the indexer cannot read JSX or nested generics; this one reads all 36 files of the sample project.
- **Tailwind -> `Style`** (`tailwind.rs`): spacing scale, the colour palette with opacity, flex / grid, sizes, borders, radius, shadows,
  gradients, positions, text. Mobile first: unprefixed classes apply; `md:`, `hover:` and the other variants do not (a phone is narrower
  than every breakpoint and has no hover), except `disabled:opacity-*`.
- **Layout** (`widgets.rs`): `flex` is a Row / Column (`gap` and `space-*` become `spacing:`), a plain `div` is a column that stretches,
  padding + background + border + radius + shadow become one `Container`, `absolute` children go in a `Stack`, `mx-auto max-w-*` is a
  centred constrained box, text styling is inherited down the tree like CSS, flex items shrink (`Flexible`). A `className` computed at
  run time goes to `Tw` (`lib/utils/tw.dart`), a small Tailwind runtime that ships with the output.
- **Components** (`component.rs`): `StatelessWidget`, or `StatefulWidget` + `State` when there is state, a ref or an effect.
  `useState` -> a field + `setState`; `useRef` -> `Ref`; `useEffect` -> a method run after the frame when its dependencies change
  (cleanup in `dispose`); `useCallback` / arrow constants -> methods; `useMemo` and other constants -> getters; props -> constructor
  parameters (read as `widget.x` in the State); `createContext` / `Provider` / `useContext` -> `ReactContext`; class components and
  plain classes -> classes.
- **Types** (`types.rs`): interfaces become classes with `copyWith`, `fromMap`, `toMap` and `[]`; `Partial<T>` becomes a class of its
  own; string-literal unions become `String` typedefs; `Promise<T>` -> `Future<T>`; `T | null` -> `T?`.
- **Routing**: `<Router><Routes><Route/>` becomes a `MaterialApp` with named routes; `useNavigate` -> `navigate(context, ...)`;
  wrappers around the router (an error boundary) move into `MaterialApp.builder`.
- **Registry** (`convert/mod.rs`): converters are registered in `CONVERTERS` like the version control commands; `--to flutter` is the
  only one so far.

## Folder mapping

| React | Flutter |
|---|---|
| `src/pages/**` | `lib/pages/**` (wrapped in a `Scaffold` + scroll view) |
| `src/components/**` | `lib/components/**` |
| `src/hooks/**`, `src/contexts/**` | `lib/hooks/**` |
| `src/utils/**`, `src/lib/**` | `lib/utils/**` |
| `src/services/**` | `lib/services/**` |
| `src/types/**` | `lib/models/**` |
| `src/data/**` | `lib/data/**` |
| `App.tsx` / `main.tsx` | `lib/app.dart` / `lib/main.dart` |

File names become snake_case (`GuardLogin.tsx` -> `guard_login.dart`, `auth.types.ts` -> `auth_types.dart`).

## What stays for a person

The report lists them (`--notes`). Browser-only APIs (`window`, `document`, `indexedDB`, `Blob`, `crypto.subtle`) and npm packages with no
Dart twin (firebase, qrcode, html5-qrcode) are written as stand-ins that throw `UnimplementedError` with the name that needs a Flutter
implementation (`lib/utils/external_stubs.dart`, `webOnly(...)` in `lib/utils/js_compat.dart`). An error boundary has no Flutter
lifecycle. `localStorage` is in memory. Types are read from the annotations; where TypeScript infers a type the converter cannot see, a
`!` or a `dynamic` may be needed (on the sample project: one analyzer error of about 7,000 lines).
