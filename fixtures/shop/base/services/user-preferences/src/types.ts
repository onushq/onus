/** Contact details and channel opt-outs for a single customer. */
export interface UserPreferences {
  userId: string;
  email: string;
  phone?: string;
  emailOptOut: boolean;
  smsOptOut: boolean;
  locale?: string;
}
