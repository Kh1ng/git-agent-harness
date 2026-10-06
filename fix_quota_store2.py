import re

with open('src/quota_store.rs', 'r') as f:
    lines = f.readlines()

out = []
skip_next = False
for i, line in enumerate(lines):
    if skip_next:
        skip_next = False
        continue
    
    if 'quota_remaining_percent: Some(100.0 - ' in line and i+1 < len(lines) and 'quota_remaining_percent: Some(' in lines[i+1]:
        # Keep the one that doesn't have 100.0 -, or just keep the next one
        out.append(lines[i+1])
        skip_next = True
    else:
        out.append(line)

with open('src/quota_store.rs', 'w') as f:
    f.writelines(out)

