export interface LocaleStrings {
  orderPlacedSubject: string;
}

const FALLBACK: LocaleStrings = {
  orderPlacedSubject: "Thanks for your order",
};

const SUPPORTED = new Set(["en-US", "en-GB", "de-DE", "fr-FR"]);

export async function loadLocaleStrings(locale: string | undefined): Promise<LocaleStrings> {
  if (!locale || !SUPPORTED.has(locale)) {
    return FALLBACK;
  }
  const strings = await import(`./locales/${locale}.js`);
  return { ...FALLBACK, ...(strings.default as Partial<LocaleStrings>) };
}
