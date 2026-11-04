// Session-only grants: microphone approval never grants camera access, and
// grants cannot survive a renderer/origin change while its dialog is open.
export function mediaPermissionPolicy({ currentContents, currentUrl, ask }) {
  const grants = new Map();
  const originOf = (contents, url) => {
    if (contents !== currentContents() || !url || !currentUrl()) return null;
    try { const origin = new URL(url).origin; return origin === new URL(currentUrl()).origin ? origin : null; }
    catch { return null; }
  };
  return {
    check(contents, permission, requestingOrigin, details = {}) {
      if (permission === 'clipboard-sanitized-write') return true;
      const origin = originOf(contents, requestingOrigin);
      return permission === 'media' && !!origin && ['audio', 'video'].includes(details.mediaType)
        && grants.get(`${contents.id}:${origin}`)?.has(details.mediaType) === true;
    },
    async request(contents, permission, callback, details = {}) {
      if (permission === 'clipboard-sanitized-write') return callback(true);
      const origin = originOf(contents, details.requestingUrl);
      const types = details.mediaTypes ?? [];
      if (permission !== 'media' || !origin || !types.length || types.some(type => !['audio', 'video'].includes(type))) return callback(false);
      const key = `${contents.id}:${origin}`;
      if (types.every(type => grants.get(key)?.has(type))) return callback(true);
      try {
        const allowed = await ask() && originOf(contents, details.requestingUrl) === origin;
        if (allowed) {
          const granted = grants.get(key) ?? new Set();
          types.forEach(type => granted.add(type)); grants.set(key, granted);
          contents.once('destroyed', () => grants.delete(key));
        }
        callback(allowed);
      } catch { callback(false); }
    },
  };
}
