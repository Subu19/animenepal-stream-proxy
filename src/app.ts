import { Hono } from "hono";
import { cors } from "hono/cors";

const app = new Hono();

// Allow all origins to use this proxy
app.use("*", cors());

app.get("/proxy", async (c) => {
  const url = decodeURIComponent(c.req.query("url") || "");
  const referer = decodeURIComponent(c.req.query("referer") || "");
  const strHeaders = c.req.query("headers") || "";
  const headersStr = strHeaders ? decodeURIComponent(strHeaders) : "";

  if (!url) {
    return c.text("URL is required", 400);
  }

  let parsedHeaders: Record<string, string> = {};
  if (headersStr) {
    try {
      parsedHeaders = JSON.parse(headersStr);
    } catch (err) {
      console.error("Proxy headers parse error:", err);
    }
  }

  if (referer && !parsedHeaders["Referer"]) {
    parsedHeaders["Referer"] = referer;
  }

  try {
    let refererUrl = parsedHeaders["Referer"] || "https://megacloud.blog/";
    let targetOrigin = "https://megacloud.blog";
    try {
      if (refererUrl) {
        targetOrigin = new URL(refererUrl).origin;
      }
    } catch (e) {
      // Ignore invalid URL
    }

    const fetchHeaders: Record<string, string> = {
      Referer: refererUrl,
      Origin: targetOrigin,
      "User-Agent":
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36",
      Accept: "*/*",
      "Accept-Language": "en-US,en;q=0.9",
      "Cache-Control": "no-cache",
      Pragma: "no-cache",
      "Sec-Ch-Ua":
        '"Chrome";v="122", "Not(A:Brand";v="24", "Google Chrome";v="122"',
      "Sec-Ch-Ua-Mobile": "?0",
      "Sec-Ch-Ua-Platform": '"Windows"',
      "Sec-Fetch-Dest": "empty",
      "Sec-Fetch-Mode": "cors",
      "Sec-Fetch-Site": "cross-site",
      "Upgrade-Insecure-Requests": "1",
      ...parsedHeaders,
    };

    const response = await fetch(url, {
      headers: fetchHeaders,
    });

    if (!response.ok) {
      console.error(
        `Proxy fetch failed for ${url} (Status: ${response.status})`,
      );
      return c.text(`Proxy fetch failed: ${response.status}`, {
        status: response.status as any,
      });
    }

    const contentType = response.headers.get("Content-Type");

    // For HLS manifests, rewrite relative URLs
    if (
      url.includes(".m3u8") ||
      contentType?.includes("application/vnd.apple.mpegurl") ||
      contentType?.includes("audio/mpegurl")
    ) {
      let text = await response.text();
      const baseUrl = url.substring(0, url.lastIndexOf("/") + 1);

      // Rewrite lines that don't start with #
      const lines = text.split("\n").map((line) => {
        const trimmed = line.trim();
        if (!trimmed || trimmed.startsWith("#")) {
          return line;
        }

        // Construct absolute URL
        let absoluteUrl: string;
        if (trimmed.startsWith("http")) {
          absoluteUrl = trimmed;
        } else {
          absoluteUrl = new URL(trimmed, baseUrl).href;
        }

        // Proxy the URL
        let proxyUrl = `${c.req.url.split("?")[0]}?url=${encodeURIComponent(absoluteUrl)}`;
        if (headersStr) {
          proxyUrl += `&headers=${encodeURIComponent(headersStr)}`;
        } else if (referer) {
          proxyUrl += `&referer=${encodeURIComponent(referer)}`;
        }
        return proxyUrl;
      });

      return c.text(lines.join("\n"), {
        headers: {
          "Content-Type": contentType || "application/vnd.apple.mpegurl",
          "Access-Control-Allow-Origin": "*",
          "Cache-Control": "no-cache",
        },
      });
    }

    // For other files (segments, etc.), pipe the stream or return as blob
    // Note: For segments, we already rewrote the URL in the manifest to point back to the proxy
    const body = await response.arrayBuffer();
    return c.body(body, {
      headers: {
        "Content-Type": contentType || "application/octet-stream",
        "Access-Control-Allow-Origin": "*",
        "Cache-Control": "max-age=3600",
      },
    });
  } catch (err: any) {
    console.error("Proxy Error:", err);
    return c.text(`Proxy Error: ${err.message}`, 500);
  }
});

export default app;
