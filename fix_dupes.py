import os
import re

for root, _, files in os.walk('.'):
    if 'node_modules' in root or '.git' in root:
        continue
    for file in files:
        if file.endswith('.ts') or file.endswith('.tsx') or file.endswith('.json'):
            path = os.path.join(root, file)
            with open(path, 'r') as f:
                content = f.read()
            # Replace occurrences of `quota_remaining_percent: <val>, quota_remaining_percent: <val2>`
            # with `quota_remaining_percent: <val2>` (since val was the used percent)
            new_content = re.sub(r'quota_remaining_percent\s*:\s*([^,]+)\s*,\s*quota_remaining_percent\s*:\s*([^,}]+)', r'quota_remaining_percent: \2', content)
            
            if new_content != content:
                with open(path, 'w') as f:
                    f.write(new_content)

