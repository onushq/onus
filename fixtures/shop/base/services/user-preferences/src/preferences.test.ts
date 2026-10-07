import { beforeEach, describe, expect, it, vi } from "vitest";

const findUnique = vi.fn();
const update = vi.fn();

vi.mock("./db", () => ({
  prisma: { userPreference: { findUnique, update } },
}));

import { getPreferences, updatePreferences } from "./preferences";

const row = {
  userId: "usr_1",
  email: "ada@example.com",
  phone: null,
  emailOptOut: false,
  smsOptOut: true,
  locale: "en-US",
};

describe("preferences", () => {
  beforeEach(() => {
    findUnique.mockReset();
    update.mockReset();
  });

  it("maps nullable columns to optional fields", async () => {
    findUnique.mockResolvedValue(row);

    const prefs = await getPreferences("usr_1");

    expect(prefs.phone).toBeUndefined();
    expect(prefs.locale).toBe("en-US");
  });

  it("throws when the user has no preferences row", async () => {
    findUnique.mockResolvedValue(null);

    await expect(getPreferences("usr_404")).rejects.toThrow("usr_404");
  });

  it("never lets a patch change the user id", async () => {
    update.mockResolvedValue({ ...row, emailOptOut: true });

    await updatePreferences("usr_1", { userId: "usr_2", emailOptOut: true });

    expect(update).toHaveBeenCalledWith({
      where: { userId: "usr_1" },
      data: { emailOptOut: true },
    });
  });
});
