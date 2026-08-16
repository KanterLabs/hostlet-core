"use client";

import { useCallback, useEffect } from "react";

export function useUnsavedChanges(dirty: boolean, message = "You have unsaved changes. Leave this page?") {
  const confirmNavigation = useCallback(() => {
    if (!dirty || typeof window === "undefined") return true;
    return window.confirm(message);
  }, [dirty, message]);

  useEffect(() => {
    if (!dirty) return;
    function onBeforeUnload(event: BeforeUnloadEvent) {
      event.preventDefault();
      event.returnValue = message;
      return message;
    }
    function onSameOriginNavigation(event: MouseEvent) {
      if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      const target = event.target instanceof Element ? event.target.closest("a[href]") : null;
      if (!(target instanceof HTMLAnchorElement) || target.target === "_blank" || target.hasAttribute("download")) return;
      const destination = new URL(target.href, window.location.href);
      if (destination.origin !== window.location.origin || destination.href === window.location.href) return;
      if (window.confirm(message)) return;
      event.preventDefault();
      event.stopPropagation();
    }
    window.addEventListener("beforeunload", onBeforeUnload);
    document.addEventListener("click", onSameOriginNavigation, true);
    return () => {
      window.removeEventListener("beforeunload", onBeforeUnload);
      document.removeEventListener("click", onSameOriginNavigation, true);
    };
  }, [dirty, message]);

  return confirmNavigation;
}
