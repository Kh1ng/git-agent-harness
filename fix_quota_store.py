import re

with open('src/quota_store.rs', 'r') as f:
    lines = f.readlines()

out = []
for line in lines:
    # Initializers
    if 'quota_used_percent: Some(' in line:
        line = re.sub(r'quota_used_percent:\s*Some\((.*?)\)', r'quota_remaining_percent: Some(100.0 - \1)', line)
    # Asserts
    elif 'assert_eq!(records[0].quota_used_percent, Some(' in line:
        line = re.sub(r'assert_eq!\(records\[0\]\.quota_used_percent,\s*Some\((.*?)\)\)', r'assert_eq!(records[0].quota_remaining_percent, Some(100.0 - \1))', line)
    elif 'assert_eq!(records[1].quota_used_percent, Some(' in line:
        line = re.sub(r'assert_eq!\(records\[1\]\.quota_used_percent,\s*Some\((.*?)\)\)', r'assert_eq!(records[1].quota_remaining_percent, Some(100.0 - \1))', line)
    elif 'assert_eq!(windows[0].quota_used_percent, Some(' in line:
        line = re.sub(r'assert_eq!\(windows\[0\]\.quota_used_percent,\s*Some\((.*?)\)\)', r'assert_eq!(windows[0].quota_remaining_percent, Some(100.0 - \1))', line)
    elif 'assert_eq!(windows[1].quota_used_percent, Some(' in line:
        line = re.sub(r'assert_eq!\(windows\[1\]\.quota_used_percent,\s*Some\((.*?)\)\)', r'assert_eq!(windows[1].quota_remaining_percent, Some(100.0 - \1))', line)
    elif 'assert_eq!(first.quota_used_percent, Some(' in line:
        line = re.sub(r'assert_eq!\(first\.quota_used_percent,\s*Some\((.*?)\)\)', r'assert_eq!(first.quota_remaining_percent, Some(100.0 - \1))', line)
    elif 'assert_eq!(second.quota_used_percent, Some(' in line:
        line = re.sub(r'assert_eq!\(second\.quota_used_percent,\s*Some\((.*?)\)\)', r'assert_eq!(second.quota_remaining_percent, Some(100.0 - \1))', line)
    elif 'assert_eq!(rec.quota_used_percent, Some(' in line:
        line = re.sub(r'assert_eq!\(rec\.quota_used_percent,\s*Some\((.*?)\)\)', r'assert_eq!(rec.quota_remaining_percent, Some(100.0 - \1))', line)
    elif 'assert_eq!(observed.quota_used_percent, Some(' in line:
        line = re.sub(r'assert_eq!\(observed\.quota_used_percent,\s*Some\((.*?)\)\)', r'assert_eq!(observed.quota_remaining_percent, Some(100.0 - \1))', line)
    elif 'assert!(rec.quota_used_percent.is_none());' in line:
        line = line.replace('assert!(rec.quota_used_percent.is_none());', 'assert!(rec.quota_remaining_percent.is_none());')
    elif 'failure.quota_used_percent = None;' in line:
        continue
    elif 'sibling.quota_used_percent = Some(99.0);' in line:
        line = line.replace('sibling.quota_used_percent = Some(99.0);', 'sibling.quota_remaining_percent = Some(1.0);')
    elif '.any(|record| record.model.is_some() && record.quota_used_percent == Some(100.0)));' in line:
        line = line.replace('.any(|record| record.model.is_some() && record.quota_used_percent == Some(100.0)));', '.any(|record| record.model.is_some() && record.quota_remaining_percent == Some(0.0)));')
    elif 'quota_used_percent,' in line and 'unwrap()' in out[-1]:
        # Multi-line assert_eq!(...unwrap().quota_used_percent, Some(70.0))
        line = line.replace('quota_used_percent,', 'quota_remaining_percent,')
    out.append(line)

with open('src/quota_store.rs', 'w') as f:
    f.writelines(out)

