import { prisma } from "./db";
import type { UserPreferences } from "./types";

interface PreferenceRow {
  userId: string;
  email: string;
  phone: string | null;
  emailOptOut: boolean;
  smsOptOut: boolean;
  locale: string | null;
}

function toPreferences(row: PreferenceRow): UserPreferences {
  return {
    userId: row.userId,
    email: row.email,
    phone: row.phone ?? undefined,
    emailOptOut: row.emailOptOut,
    smsOptOut: row.smsOptOut,
    locale: row.locale ?? undefined,
  };
}

export async function getPreferences(userId: string): Promise<UserPreferences> {
  const row = await prisma.userPreference.findUnique({ where: { userId } });
  if (!row) {
    throw new Error(`No preferences for user ${userId}`);
  }
  return toPreferences(row);
}

export async function updatePreferences(
  userId: string,
  patch: Partial<UserPreferences>,
): Promise<UserPreferences> {
  const { userId: _ignored, ...data } = patch;
  const row = await prisma.userPreference.update({ where: { userId }, data });
  return toPreferences(row);
}
