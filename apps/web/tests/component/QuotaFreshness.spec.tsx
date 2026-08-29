import { test, expect } from "@playwright/experimental-ct-react";
import { QuotaFreshnessPanel } from "../../src/pages/QuotaPage.js";
import React from "react";

test("renders recent no-data checks separately from stale quota data and exposes failures", async ({
  mount,
}) => {
  const recent = new Date().toISOString();
  const stale = new Date(Date.now() - 3 * 24 * 60 * 60 * 1000).toISOString();
  const component = await mount(
    <QuotaFreshnessPanel
      generatedAt={recent}
      freshness={{ quota_checked_at: recent, quota_observed_at: stale }}
      quotaChecks={[
        { backend: "codex", checked_at: recent, status: "no_data" },
        {
          backend: "vibe",
          checked_at: stale,
          status: "failed",
          error: "Mistral Admin API unavailable",
        },
      ]}
    />,
  );

  await expect(
    component.getByText("Account quota check", { exact: true }),
  ).toBeVisible();
  await expect(
    component.getByText("Quota data", { exact: true }),
  ).toBeVisible();
  await expect(component.getByText("No quota data recorded")).toHaveClass(
    /badge-unknown/,
  );
  await expect(component.getByText("Check failed")).toBeVisible();
  await expect(
    component.getByText("Mistral Admin API unavailable"),
  ).toBeVisible();
  await expect(
    component.getByTestId("quota-check-codex").getByText("Stale"),
  ).toHaveCount(0);
  await expect(
    component.getByTestId("quota-check-vibe").getByText("Stale"),
  ).toBeVisible();
});
