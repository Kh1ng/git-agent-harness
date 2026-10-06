with open("src/quota_store.rs", "r") as f:
    lines = f.readlines()

out = []
docstring_start = -1
docstring_end = -1
for i, line in enumerate(lines):
    if line.startswith("/// Load readable records"):
        docstring_start = i
    if line.startswith("fn normalize_quota_percent"):
        normalize_start = i
        break

# We know normalize_quota_percent is right after docstring
# Let's just find the function block
normalize_block = []
in_fn = False
fn_braces = 0
fn_start = -1
fn_end = -1
for i, line in enumerate(lines):
    if line.startswith("fn normalize_quota_percent"):
        in_fn = True
        fn_start = i
    if in_fn:
        fn_braces += line.count("{")
        fn_braces -= line.count("}")
        if fn_braces == 0:
            in_fn = False
            fn_end = i
            break

# docstring is lines 140, 141 (0-indexed). Actually let's just do it directly.
