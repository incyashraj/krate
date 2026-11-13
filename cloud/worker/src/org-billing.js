// Organisations are not billed (founder, 2026-11-07: Krate has no paid
// plans). The billing routes that once existed answer 410 Gone with plain
// words, so an old console or script hears why instead of a 404, and
// nothing is read, written or sent anywhere.

const GONE = {
  error: "no-billing",
  message: "Krate has no paid plans. Nothing about an organisation is billed.",
};

/// /orgs/<slug>/billing and anything under it, any method: 410. Anything
/// else is not this module's (null).
export async function orgBillingRoute(h) {
  const { rest } = h;
  if (rest[0] !== "billing") return null;
  return h.json(GONE, 410);
}
