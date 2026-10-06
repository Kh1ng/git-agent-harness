sed -i 's/quota_used_percent: .*//g' src/usage.rs
sed -i 's/quota_used_percent,//g' src/usage.rs
sed -i 's/usage.quota_used_percent/usage.quota_remaining_percent/g' src/usage.rs
