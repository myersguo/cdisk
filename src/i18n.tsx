import { createContext, useContext, useEffect, useState } from "react";
import {
  getLocale,
  LOCALE_OPTIONS,
  resolveLocale,
  setCurrentLocale,
  t,
  type Locale,
  type MessageKey,
} from "./locales";

export { LOCALE_OPTIONS, resolveLocale, t };
export type { Locale, MessageKey };

const LocaleContext = createContext<{
  locale: Locale;
  setLocale: (locale: Locale) => void;
}>({ locale: getLocale(), setLocale: () => {} });

export function LocaleProvider({ children }: { children: React.ReactNode }) {
  const [locale, updateLocale] = useState<Locale>(getLocale());

  useEffect(() => {
    setCurrentLocale(locale);
  }, [locale]);

  function setLocale(next: Locale) {
    setCurrentLocale(next);
    try {
      localStorage.setItem("cdisk.locale", next);
    } catch {
      // Persistence is best-effort; the selected locale still applies now.
    }
    updateLocale(next);
  }

  return <LocaleContext.Provider value={{ locale, setLocale }}>{children}</LocaleContext.Provider>;
}

export function useLocale() {
  return useContext(LocaleContext);
}
