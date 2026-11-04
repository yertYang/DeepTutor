import { NextRequest, NextResponse } from "next/server";
import { resolveBackendApiBase } from "./lib/backend-runtime-config";
import { prepareBackendForwardHeaders } from "./lib/backend-forward";
import {
  CODEX_CALLBACK_API_PATH,
  COOKIE_NAME,
  LOGIN_PATH,
  isAuthExempt,
  isBackendPath,
  isCodexCallbackPath,
  isRetiredPagePath,
  isWebSocketPath,
} from "./lib/proxy-policy";

// Backend base URL for `/api/*` and `/ws/*` rewrites. The native launcher
// exports `DEEPTUTOR_API_BASE_URL` using the backend's listening address.
// The resolver also supports BACKEND_PORT and NEXT_PUBLIC_API_BASE for
// standalone frontend development.
//
// The loopback is spelled as the IPv4 literal, not `localhost`: on a dual-stack
// host that name may resolve to ::1 first, while the native launcher binds
// the backend to 127.0.0.1, causing an IPv6 connection to fail.
const API_BASE_URL = resolveBackendApiBase();

// Redirect to the login page, preserving the intended destination in `next`.
// A present-but-invalid cookie is cleared so the browser stops resending it;
// when no cookie was sent there is nothing to clear.
function redirectToLogin(
  req: NextRequest,
  { clearCookie }: { clearCookie: boolean },
): NextResponse {
  const loginUrl = req.nextUrl.clone();
  loginUrl.pathname = LOGIN_PATH;
  loginUrl.search = "";
  loginUrl.searchParams.set(
    "next",
    `${req.nextUrl.pathname}${req.nextUrl.search}`,
  );
  const response = NextResponse.redirect(loginUrl);
  if (clearCookie) response.cookies.delete(COOKIE_NAME);
  return response;
}

export async function proxy(req: NextRequest): Promise<NextResponse> {
  const { pathname, search } = req.nextUrl;

  if (isCodexCallbackPath(pathname)) {
    return NextResponse.rewrite(
      new URL(CODEX_CALLBACK_API_PATH + search, API_BASE_URL),
    );
  }

  if (isRetiredPagePath(pathname)) {
    return new NextResponse(null, { status: 404 });
  }

  // 1. Bridge the origin gap: forward backend-relative paths to the API server.
  //    This keeps the URL knowledge in one place (the entrypoint + system.json)
  //    rather than baked into the frontend bundle.
  if (isBackendPath(pathname)) {
    return NextResponse.rewrite(new URL(pathname + search, API_BASE_URL), {
      request: {
        headers: prepareBackendForwardHeaders(req.headers, {
          allowWebSocketUpgrade:
            req.method === "GET" && isWebSocketPath(pathname),
        }),
      },
    });
  }

  if (isAuthExempt(pathname)) {
    return NextResponse.next();
  }

  // Ask the running backend, including after enabling multi-user mode without
  // a rebuild. A cookie's presence or unverified payload is never authority.
  try {
    const response = await fetch(new URL("/api/auth/status", API_BASE_URL), {
      headers: { cookie: req.headers.get("cookie") ?? "" },
      cache: "no-store",
      signal: AbortSignal.timeout(5000),
    });
    if (!response.ok) throw new Error("Authentication unavailable");
    const status = await response.json();
    if (status.enabled && !status.authenticated) {
      return redirectToLogin(req, { clearCookie: Boolean(req.cookies.get(COOKIE_NAME)) });
    }
  } catch {
    return new NextResponse("Authentication service unavailable", { status: 503 });
  }

  return NextResponse.next();
}

export const config = {
  // Run on every request except Next.js internals and the favicon. The /api/*
  // and /ws/* paths are explicitly handled above (rewritten to the backend);
  // Speech recordings, knowledge-base creation and upload requests with multipart bodies
  // are handled by dedicated App Router endpoints that stream directly to
  // the backend. The browser's list call uses /api/knowledge-bases/list so it can
  // pass through the normal backend rewrite below. Excluding multipart routes
  // here is crucial:
  // merely entering Proxy makes Next clone and cap the multipart body.
  // the browser's /_next/image optimizer requests are excluded here, while the
  // optimizer's loopback fetch for the source image (e.g. /logo.png) is let
  // through the auth gate by isAuthExempt.
  matcher: [
    "/((?!_next/static|_next/image|favicon.ico|api/knowledge-bases(?:$|/[^/]+/upload(?:/|$))|api/browser-testing/sources$|api/voice/stt$).*)",
  ],
};
