// The ~20 edit tasks: small multi-file repos, a user-style prompt, and a
// host-side verifier. Kept plain-data so a failed task can be re-run by id.

pub struct Task {
    pub id: &'static str,
    /// (path, content) seeds of the mini repo
    pub files: &'static [(&'static str, &'static str)],
    pub prompt: &'static str,
    /// (path, substring that must exist afterwards) — presence checks; a
    /// path prefixed with '!' must NOT contain the substring.
    pub expect: &'static [(&'static str, &'static str)],
}

pub const TASKS: &[Task] = &[
    // --- rust -------------------------------------------------------------
    Task {
        id: "rs-rename-fn",
        files: &[
            (
                "src/lib.rs",
                "pub fn total(items: &[i32]) -> i32 {\n    items.iter().sum()\n}\n",
            ),
            (
                "src/main.rs",
                "use demo::total;\n\nfn main() {\n    println!(\"{}\", total(&[1, 2]));\n}\n",
            ),
        ],
        prompt: "Rename the `total` function to `sum` everywhere.",
        expect: &[
            ("src/lib.rs", "pub fn sum"),
            ("src/main.rs", "sum(&[1, 2])"),
            ("!src/main.rs", "total("),
        ],
    },
    Task {
        id: "rs-fix-compile",
        files: &[(
            "src/lib.rs",
            "pub fn half(x: i32) -> i32 {\n    x / 2\n}\n\npub fn broken() -> i32 {\n    half(5\n}\n",
        )],
        prompt: "src/lib.rs does not compile. Fix the syntax error, keep the logic.",
        expect: &[("src/lib.rs", "half(5)"), ("src/lib.rs", "pub fn broken")],
    },
    Task {
        id: "rs-extract-struct",
        files: &[(
            "src/main.rs",
            "fn main() {\n    let name = \"a\";\n    let age = 3u8;\n    println!(\"{name} {age}\");\n}\n",
        )],
        prompt: "Extract the name/age pair into a `User` struct with those two fields and use it in main.",
        expect: &[
            ("src/main.rs", "struct User"),
            ("src/main.rs", "name: String"),
        ],
    },
    Task {
        id: "rs-add-tests",
        files: &[(
            "src/lib.rs",
            "pub fn clamp01(v: f64) -> f64 {\n    v.clamp(0.0, 1.0)\n}\n",
        )],
        prompt: "Add a #[cfg(test)] module with a test that clamp01(2.0) == 1.0.",
        expect: &[
            ("src/lib.rs", "#[cfg(test)]"),
            ("src/lib.rs", "clamp01(2.0)"),
        ],
    },
    Task {
        id: "rs-move-fn",
        files: &[
            ("src/lib.rs", "pub mod a;\npub mod b;\n"),
            (
                "src/a.rs",
                "pub fn greet() -> &'static str {\n    \"hi\"\n}\n",
            ),
            (
                "src/b.rs",
                "use crate::a::greet;\n\npub fn say() -> String {\n    greet().to_string()\n}\n",
            ),
        ],
        prompt: "Move `greet` from module a to module b and fix all imports so the code still compiles.",
        expect: &[
            ("src/b.rs", "pub fn greet"),
            ("!src/a.rs", "fn greet"),
            ("src/b.rs", "pub fn say"),
        ],
    },
    // --- python -----------------------------------------------------------
    Task {
        id: "py-rename",
        files: &[
            (
                "app/calc.py",
                "def add_vat(amount):\n    return amount * 1.2\n",
            ),
            (
                "app/main.py",
                "from app.calc import add_vat\n\nprint(add_vat(100))\n",
            ),
        ],
        prompt: "Rename add_vat to with_vat everywhere.",
        expect: &[
            ("app/calc.py", "def with_vat"),
            ("app/main.py", "with_vat(100)"),
            ("!app/main.py", "add_vat"),
        ],
    },
    Task {
        id: "py-fix-indent",
        files: &[(
            "app/mod.py",
            "def f(xs):\n      total = 0\n        for x in xs:\n        total += x\n    return total\n",
        )],
        prompt: "app/mod.py has broken indentation and will not run. Fix it.",
        expect: &[
            ("app/mod.py", "    total = 0"),
            ("app/mod.py", "for x in xs:"),
        ],
    },
    Task {
        id: "py-docstring",
        files: &[(
            "app/db.py",
            "def connect(url):\n    return {\"url\": url}\n",
        )],
        prompt: "Add a docstring to connect() explaining it returns a config dict for the given database url.",
        expect: &[("app/db.py", "\"\"\"")],
    },
    Task {
        id: "py-add-arg",
        files: &[(
            "app/run.py",
            "def greet(name):\n    return \"hello \" + name\n",
        )],
        prompt: "Add an optional `loud: bool = False` parameter; when True, return the greeting uppercased.",
        expect: &[("app/run.py", "loud"), ("app/run.py", ".upper()")],
    },
    Task {
        id: "py-split",
        files: &[(
            "app/util.py",
            "import json\n\ndef load(p):\n    with open(p) as f:\n        return json.load(f)\n\ndef dump(p, data):\n    with open(p, \"w\") as f:\n        json.dump(data, f)\n",
        )],
        prompt: "Split app/util.py into io_util.py (load/dump) and keep util.py re-exporting both functions.",
        expect: &[("app/io_util.py", "def load"), ("app/util.py", "import")],
    },
    // --- javascript / typescript ------------------------------------------
    Task {
        id: "js-const",
        files: &[(
            "web/math.mjs",
            "function area(r) {\n  return 3.14159 * r * r;\n}\nexport { area };\n",
        )],
        prompt: "Replace the magic number in area() with a named constant PI at the top of the file.",
        expect: &[
            ("web/math.mjs", "const PI"),
            ("web/math.mjs", "PI * r * r"),
            ("!web/math.mjs", "3.14159"),
        ],
    },
    Task {
        id: "js-default-export",
        files: &[
            (
                "web/api.mjs",
                "export function ping() {\n  return \"pong\";\n}\n",
            ),
            (
                "web/app.mjs",
                "import { ping } from \"./api.mjs\";\nconsole.log(ping());\n",
            ),
        ],
        prompt: "Change ping() to a default export and update the import in app.mjs.",
        expect: &[
            ("web/api.mjs", "export default"),
            ("web/app.mjs", "import ping"),
            ("!web/app.mjs", "import { ping }"),
        ],
    },
    Task {
        id: "ts-types",
        files: &[(
            "web/sum.ts",
            "export function sum(xs) {\n  return xs.reduce((a, b) => a + b, 0);\n}\n",
        )],
        prompt: "Add TypeScript types: xs is number[], the function returns number.",
        expect: &[("web/sum.ts", "xs: number[]"), ("web/sum.ts", ": number")],
    },
    Task {
        id: "ts-interface",
        files: &[(
            "web/cart.ts",
            "export function total(cart) {\n  return cart.items.map((i) => i.price).reduce((a, b) => a + b, 0);\n}\n",
        )],
        prompt: "Introduce `interface CartItem { price: number }` and `interface Cart { items: CartItem[] }` and type the cart parameter.",
        expect: &[
            ("web/cart.ts", "interface CartItem"),
            ("web/cart.ts", "interface Cart"),
        ],
    },
    // --- json / docs ------------------------------------------------------
    Task {
        id: "json-field",
        files: &[(
            "cfg/app.json",
            "{\n  \"name\": \"demo\",\n  \"debug\": false\n}\n",
        )],
        prompt: "Add a top-level \"retries\": 3 field to cfg/app.json and flip \"debug\" to true.",
        expect: &[
            ("cfg/app.json", "\"retries\": 3"),
            ("cfg/app.json", "\"debug\": true"),
        ],
    },
    Task {
        id: "md-link",
        files: &[(
            "README.md",
            "# demo\n\nSee the guide for details.\n\n## Guide\n\nText.\n",
        )],
        prompt: "Turn the words 'the guide' in the first paragraph into a markdown link pointing to #guide.",
        expect: &[("README.md", "[the guide](#guide)")],
    },
    // --- c / go / java ----------------------------------------------------
    Task {
        id: "c-rename",
        files: &[
            (
                "src/mathx.c",
                "int add(int a, int b) {\n    return a + b;\n}\n",
            ),
            (
                "src/main.c",
                "int add(int a, int b);\n\nint main(void) {\n    return add(1, 2);\n}\n",
            ),
        ],
        prompt: "Rename add to add_ints in both files (declaration, definition, call).",
        expect: &[
            ("src/mathx.c", "int add_ints"),
            ("src/main.c", "add_ints(1, 2)"),
            ("!src/main.c", "int add("),
        ],
    },
    Task {
        id: "go-rename",
        files: &[
            (
                "app/util.go",
                "package app\n\nfunc Shout(s string) string {\n    return s + \"!\"\n}\n",
            ),
            (
                "app/main.go",
                "package app\n\nfunc Run() string {\n    return Shout(\"hi\")\n}\n",
            ),
        ],
        prompt: "Rename Shout to Exclaim in both files.",
        expect: &[
            ("app/util.go", "func Exclaim"),
            ("app/main.go", "Exclaim(\"hi\")"),
        ],
    },
    Task {
        id: "java-rename",
        files: &[
            (
                "src/Greeter.java",
                "public class Greeter {\n    public String hello() {\n        return \"hi\";\n    }\n}\n",
            ),
            (
                "src/Main.java",
                "public class Main {\n    public static void main(String[] args) {\n        System.out.println(new Greeter().hello());\n    }\n}\n",
            ),
        ],
        prompt: "Rename Greeter.hello() to greet() and update the call site.",
        expect: &[
            ("src/Greeter.java", "public String greet()"),
            ("src/Main.java", ".greet()"),
        ],
    },
    // --- bash-free workflows -----------------------------------------------
    Task {
        id: "multi-rename",
        files: &[
            (
                "src/lib.rs",
                "pub mod worker;\n\npub fn run() {\n    worker::tick();\n}\n",
            ),
            (
                "src/worker.rs",
                "pub fn tick() {\n    beat();\n}\n\nfn beat() {}\n",
            ),
        ],
        prompt: "Rename the private function beat() in worker.rs to pulse(), keeping tick() working.",
        expect: &[
            ("src/worker.rs", "fn pulse()"),
            ("src/worker.rs", "pulse();"),
            ("!src/lib.rs", "pulse"),
        ],
    },
];
