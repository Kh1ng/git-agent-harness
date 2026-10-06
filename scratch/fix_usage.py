import re

with open("src/usage.rs", "r") as f:
    content = f.read()

# Fix parsing logic
content = content.replace(
    '    let quota_remaining_percent = find_f64(\n        text,\n        &["quota_remaining_percent", "quota remaining percent"],\n    );',
    '    let quota_remaining_percent = find_f64(\n        text,\n        &["quota_remaining_percent", "quota remaining percent"],\n    ).or_else(|| quota_used_percent.map(|used| 100.0 - used));'
)

# Fix LedgerUsage init
content = content.replace("        quota_used_percent,\n", "")

# Fix Bucket init
content = content.replace("            used_percent,\n", "")
content = content.replace("        used_percent: f64,\n", "")

# Fix tests
def replace_test(match):
    val = float(match.group(1))
    new_val = 100.0 - val
    return f"assert_eq!(usage.quota_remaining_percent, Some({new_val}));"

content = re.sub(r'assert_eq!\(usage\.quota_used_percent, Some\(([0-9.]+)\)\);', replace_test, content)

def replace_test_windows(match):
    val = float(match.group(1))
    new_val = 100.0 - val
    return f"assert_eq!(windows[0].quota_remaining_percent, Some({new_val}));"

content = re.sub(r'assert_eq!\(windows\[0\]\.quota_used_percent, Some\(([0-9.]+)\)\);', replace_test_windows, content)

content = content.replace("usage.quota_used_percent, None", "usage.quota_remaining_percent, None")
content = content.replace("windows[0].quota_used_percent, None", "windows[0].quota_remaining_percent, None")

with open("src/usage.rs", "w") as f:
    f.write(content)

