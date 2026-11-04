import { forwardBackendUpload } from "@/lib/streaming-upload-proxy";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

// A provider/configuration rejection can arrive before the audio upload ends.
// Use fetch's streaming transport instead of Next's http-proxy rewrite, which
// can replace that response with HTTP 500 after an EPIPE on its upload socket.
export async function POST(request: Request): Promise<Response> {
  try {
    return await forwardBackendUpload(request);
  } catch {
    const message = "Unable to upload the recording. Check the application connection and try again.";
    return Response.json({ error_code: "speech_upload_failed", message, detail: message, retryable: true }, { status: 502 });
  }
}
