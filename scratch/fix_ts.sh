sed -i '/export interface QuotaListRecord {/,/}/d' packages/contracts/src/gah.ts
sed -i 's/QuotaListRecord/QuotaObservation/g' packages/contracts/src/gah.ts apps/web/src/api/client.ts
