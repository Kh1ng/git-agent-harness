/**
 * Compile-time drift checks for checked CLI fixtures.
 *
 * Regenerate after an intentional Rust JSON contract change with:
 * `GAH_UPDATE_CONTRACT_FIXTURES=1 cargo test --test contracts_drift`
 */
import quotaListFixture from './fixtures/quota-list.json';
import reportFixture from './fixtures/report.json';
import statusFixture from './fixtures/status.json';

import type { QuotaObservation, ReportData, StatusSnapshot } from './gah.js';

// TypeScript intentionally widens string literals and tuple-shaped arrays in
// imported JSON modules. Preserve the contract's required keys and scalar
// kinds while representing that JSON-module inference faithfully.
type JsonModuleShape<Expected> = Expected extends string
  ? string
  : Expected extends number
    ? number
    : Expected extends boolean
      ? boolean
      : Expected extends null
        ? null
        : Expected extends readonly unknown[]
          ? JsonModuleShape<Expected[number]>[]
          : Expected extends object
            ? { [Key in keyof Expected]: JsonModuleShape<Expected[Key]> }
            : Expected;

const checkedStatus = statusFixture satisfies JsonModuleShape<StatusSnapshot>;
const checkedQuotaList = quotaListFixture satisfies JsonModuleShape<QuotaObservation[]>;
const checkedReport = reportFixture satisfies JsonModuleShape<ReportData>;

void checkedStatus;
void checkedQuotaList;
void checkedReport;

type NoExtraFieldsDeep<Actual, Expected> = Actual extends readonly unknown[]
  ? Expected extends readonly unknown[]
    ? NoExtraFieldsDeep<Actual[number], Expected[number]>
    : false
  : Actual extends object
    ? Expected extends object
      ? Exclude<keyof Actual, keyof Expected> extends never
        ? {
            [Key in keyof Actual]: Key extends keyof Expected
              ? NoExtraFieldsDeep<Actual[Key], NonNullable<Expected[Key]>>
              : false;
          }[keyof Actual] extends true
          ? true
          : false
        : false
      : false
    : true;

type AssertTrue<Value extends true> = Value;

type StatusFixtureHasNoExtraFields = AssertTrue<
  NoExtraFieldsDeep<typeof statusFixture, StatusSnapshot>
>;
type QuotaFixtureHasNoExtraFields = AssertTrue<
  NoExtraFieldsDeep<typeof quotaListFixture, QuotaObservation[]>
>;
type ReportFixtureHasNoExtraFields = AssertTrue<
  NoExtraFieldsDeep<typeof reportFixture, ReportData>
>;

const fixtureWithUnexpectedNestedField = {
  ...statusFixture,
  profile: { ...statusFixture.profile, unexpected_contract_field: true },
};

// This is a live regression proof: removing the recursive excess-field check
// makes the expected compiler error disappear and fails the TypeScript build.
// @ts-expect-error nested fixture fields absent from the contract are rejected
type UnexpectedNestedFieldIsRejected = AssertTrue<NoExtraFieldsDeep<typeof fixtureWithUnexpectedNestedField, StatusSnapshot>>;

export type ContractFixtureChecks =
  | StatusFixtureHasNoExtraFields
  | QuotaFixtureHasNoExtraFields
  | ReportFixtureHasNoExtraFields
  | UnexpectedNestedFieldIsRejected;
