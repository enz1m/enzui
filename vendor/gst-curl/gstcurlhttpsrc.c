/*
 * GstEnzCurlHttpSrc
 * Copyright 2017 British Broadcasting Corporation - Research and Development
 *
 * Author: Sam Hurst <samuelh@rd.bbc.co.uk>
 *
 * Permission is hereby granted, free of charge, to any person obtaining a
 * copy of this software and associated documentation files (the "Software"),
 * to deal in the Software without restriction, including without limitation
 * the rights to use, copy, modify, merge, publish, distribute, sublicense,
 * and/or sell copies of the Software, and to permit persons to whom the
 * Software is furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
 * FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
 * DEALINGS IN THE SOFTWARE.
 *
 * Alternatively, the contents of this file may be used under the
 * GNU Lesser General Public License Version 2.1 (the "LGPL"), in
 * which case the following provisions apply instead of the ones
 * mentioned above:
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Library General Public
 * License as published by the Free Software Foundation; either
 * version 2 of the License, or (at your option) any later version.
 *
 * This library is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Library General Public License for more details.
 *
 * You should have received a copy of the GNU Library General Public
 * License along with this library; if not, write to the
 * Free Software Foundation, Inc., 59 Temple Place - Suite 330,
 * Boston, MA 02111-1307, USA.
 */

/* Private HTTP/1.1 source; routing and peer policy are owned by the Rust adapter. */

/* Each source owns its Curl multi task. Acquire the multi-context mutex before
 * the buffer mutex; release the buffer mutex before Curl can reenter callbacks.
 */

#ifdef HAVE_CONFIG_H
#include <config.h>
#endif

#include <glib/gi18n-lib.h>
#ifdef __linux__
#include <errno.h>
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>
#endif

#include "gstcurlhttpsrc.h"
#include "gstcurlqueue.h"
#include "gstcurldefaults.h"

GST_DEBUG_CATEGORY_STATIC (enzui_curl_http_src_debug);
#define GST_CAT_DEFAULT enzui_curl_http_src_debug
GST_DEBUG_CATEGORY_STATIC (enzui_curl_loop_debug);

#define CURL_HTTP_SRC_ERROR(src,cat,code,error_message)     \
  do { \
    GST_ELEMENT_ERROR_WITH_DETAILS ((src), cat, code, ("%s", error_message), \
        ("%s (%d), URL: %s, Redirect to: %s", (src)->reason_phrase, \
            (src)->status_code, (src)->uri, GST_STR_NULL ((src)->redirect_uri)), \
            ("http-status-code", G_TYPE_UINT, (src)->status_code, \
             "http-redirect-uri", G_TYPE_STRING, GST_STR_NULL ((src)->redirect_uri), NULL)); \
  } while(0)

enum
{
  PROP_0,
  PROP_URI,
  PROP_USERNAME,
  PROP_PASSWORD,
  PROP_PROXYURI,
  PROP_PROXYUSERNAME,
  PROP_PROXYPASSWORD,
  PROP_COOKIES,
  PROP_USERAGENT,
  PROP_HEADERS,
  PROP_COMPRESS,
  PROP_REDIRECT,
  PROP_MAXREDIRECT,
  PROP_KEEPALIVE,
  PROP_TIMEOUT,
  PROP_STRICT_SSL,
  PROP_SSL_CA_FILE,
  PROP_RETRIES,
  PROP_CONNECTIONMAXTIME,
  PROP_MAXCONCURRENT_SERVER,
  PROP_MAXCONCURRENT_PROXY,
  PROP_MAXCONCURRENT_GLOBAL,
  PROP_HTTPVERSION,
  PROP_RECEIVE_LIMIT,
  PROP_BUFFERED_BYTES,
  PROP_PEAK_BUFFERED_BYTES,
  PROP_BACKPRESSURE_COUNT,
  PROP_RECEIVE_PAUSED,
  PROP_READ_AHEAD_CAPACITY,
  PROP_READ_AHEAD_FD,
  PROP_READ_AHEAD_READY,
  PROP_READ_AHEAD_BUFFERED,
  PROP_READ_AHEAD_PEAK,
  PROP_READ_AHEAD_FILE_BYTES,
  PROP_IRADIO_MODE,
  PROP_MAX
};

/*
 * Make a source pad template to be able to kick out recv'd data
 */
static GstStaticPadTemplate srcpadtemplate = GST_STATIC_PAD_TEMPLATE ("src",
    GST_PAD_SRC,
    GST_PAD_ALWAYS,
    GST_STATIC_CAPS_ANY);

/*
 * Function Definitions
 */
/* Gstreamer generic element functions */
static void enzui_curl_http_src_set_property (GObject * object, guint prop_id,
    const GValue * value, GParamSpec * pspec);
static void enzui_curl_http_src_get_property (GObject * object, guint prop_id,
    GValue * value, GParamSpec * pspec);
static gboolean enzui_curl_http_src_ref_multi (GstEnzCurlHttpSrc * src);
static void enzui_curl_http_src_unref_multi (GstEnzCurlHttpSrc * src);
static void enzui_curl_http_src_finalize (GObject * obj);
static GstFlowReturn enzui_curl_http_src_create (GstPushSrc * psrc,
    GstBuffer ** outbuf);
static GstFlowReturn enzui_curl_http_src_handle_response (GstEnzCurlHttpSrc * src);
static gboolean enzui_curl_http_src_negotiate_caps (GstEnzCurlHttpSrc * src);
static GstStateChangeReturn enzui_curl_http_src_change_state (GstElement *
    element, GstStateChange transition);
static void enzui_curl_http_src_cleanup_instance (GstEnzCurlHttpSrc * src);
static gboolean enzui_curl_http_src_query (GstBaseSrc * bsrc, GstQuery * query);
static gboolean enzui_curl_http_src_get_size (GstBaseSrc * bsrc, guint64 * size);
static gboolean enzui_curl_http_src_is_seekable (GstBaseSrc * bsrc);
static gboolean enzui_curl_http_src_do_seek (GstBaseSrc * bsrc,
    GstSegment * segment);
static gboolean enzui_curl_http_src_unlock (GstBaseSrc * bsrc);
static gboolean enzui_curl_http_src_unlock_stop (GstBaseSrc * bsrc);
static guint64 enzui_curl_http_src_get_length_from_headers (GstEnzCurlHttpSrc * src);

/* URI Handler functions */
static void enzui_curl_http_src_uri_handler_init (gpointer g_iface,
    gpointer iface_data);
static guint enzui_curl_http_src_urihandler_get_type (GType type G_GNUC_UNUSED);
static const gchar *const *enzui_curl_http_src_urihandler_get_protocols (GType
    type);
static gchar *enzui_curl_http_src_urihandler_get_uri (GstURIHandler * handler);
static gboolean enzui_curl_http_src_urihandler_set_uri (GstURIHandler * handler,
    const gchar * uri, GError ** error G_GNUC_UNUSED);

/* GstTask functions */
static void enzui_curl_http_src_curl_multi_loop (gpointer thread_data);
static CURL *enzui_curl_http_src_create_easy_handle (GstEnzCurlHttpSrc * s);
static inline void enzui_curl_http_src_destroy_easy_handle (GstEnzCurlHttpSrc * src);
static size_t enzui_curl_http_src_get_header (void *header, size_t size,
    size_t nmemb, void *src);
static size_t enzui_curl_http_src_get_chunks (void *chunk, size_t size,
    size_t nmemb, void *src);
static void enzui_curl_http_src_request_remove (GstEnzCurlHttpSrc * src);
static void enzui_curl_http_src_wait_until_removed (GstEnzCurlHttpSrc * src);

static curl_version_info_data *enzui_curl_http_src_curl_capabilities = NULL;

#define GST_TYPE_CURL_HTTP_VERSION (enzui_curl_http_version_get_type ())

static GType
enzui_curl_http_version_get_type (void)
{
  static GType gtype = 0;

  if (!gtype) {
    static const GEnumValue http_versions[] = {
      {GSTCURL_HTTP_VERSION_1_0, "HTTP Version 1.0", "1.0"},
      {GSTCURL_HTTP_VERSION_1_1, "HTTP Version 1.1", "1.1"},
#ifdef CURL_VERSION_HTTP2
      {GSTCURL_HTTP_VERSION_2_0, "HTTP Version 2.0", "2.0"},
#endif
      {0, NULL, NULL}
    };
    gtype = g_enum_register_static ("GstEnzCurlHttpVersionType", http_versions);
  }
  return gtype;
}

#define enzui_curl_http_src_parent_class parent_class
G_DEFINE_TYPE_WITH_CODE (GstEnzCurlHttpSrc, enzui_curl_http_src, GST_TYPE_PUSH_SRC,
    G_IMPLEMENT_INTERFACE (GST_TYPE_URI_HANDLER,
        enzui_curl_http_src_uri_handler_init));
#define ENZUI_RECEIVE_LIMIT (256U * 1024U)


static gboolean
enzui_read_ahead_enabled (GstEnzCurlHttpSrc *src)
{
  return src->read_ahead_fd >= 0 && src->read_ahead_capacity >= ENZUI_RECEIVE_LIMIT;
}

static guint
enzui_receive_capacity (GstEnzCurlHttpSrc *src)
{
  return enzui_read_ahead_enabled (src) ? src->read_ahead_capacity : ENZUI_RECEIVE_LIMIT;
}

static void
enzui_clear_body (GstEnzCurlHttpSrc *src)
{
  g_clear_pointer (&src->buffer, g_free);
  src->buffer_len = 0;
  src->read_ahead_read = 0;
  src->read_ahead_write = 0;
}

static void
enzui_close_read_ahead (GstEnzCurlHttpSrc *src)
{
#ifdef __linux__
  if (src->read_ahead_fd >= 0)
    close (src->read_ahead_fd);
#endif
  src->read_ahead_fd = -1;
}

static void
enzui_import_read_ahead (GstEnzCurlHttpSrc *src, gint fd)
{
  enzui_close_read_ahead (src);
  enzui_clear_body (src);
#ifdef __linux__
  if (fd >= 0 && src->read_ahead_capacity >= ENZUI_RECEIVE_LIMIT) {
    gint imported = fcntl (fd, F_DUPFD_CLOEXEC, 3);
    struct stat metadata;
    if (imported >= 0 && fstat (imported, &metadata) == 0 &&
        S_ISREG (metadata.st_mode) && metadata.st_nlink == 0 &&
        metadata.st_uid == geteuid () && (metadata.st_mode & 077) == 0 &&
        metadata.st_size == (off_t) src->read_ahead_capacity &&
        (fcntl (imported, F_GETFL) & O_ACCMODE) == O_RDWR) {
      src->read_ahead_fd = imported;
    } else if (imported >= 0) {
      close (imported);
    }
  }
#else
  (void) fd;
#endif
}

/* Callers hold buffer_mutex; consumed bytes are never advertised as cached ranges. */
static gboolean
enzui_read_ahead_io (GstEnzCurlHttpSrc *src, guint offset, guint length,
    gchar *bytes, gboolean write_data)
{
#ifdef __linux__
  guint completed = 0;
  while (completed < length) {
    guint position = (guint) (((guint64) offset + completed) % src->read_ahead_capacity);
    guint block = MIN (length - completed, src->read_ahead_capacity - position);
    ssize_t result = write_data ?
        pwrite (src->read_ahead_fd, bytes + completed, block, (off_t) position) :
        pread (src->read_ahead_fd, bytes + completed, block, (off_t) position);
    if (result < 0 && errno == EINTR)
      continue;
    if (result <= 0)
      return FALSE;
    completed += (guint) result;
  }
  return TRUE;
#else
  (void) src; (void) offset; (void) length; (void) bytes; (void) write_data;
  return FALSE;
#endif
}

static void
enzui_curl_http_src_class_init (GstEnzCurlHttpSrcClass * klass)
{
  GObjectClass *gobject_class;
  GstElementClass *gstelement_class;
  GstBaseSrcClass *gstbasesrc_class;
  GstPushSrcClass *gstpushsrc_class;

  gobject_class = (GObjectClass *) klass;
  gstelement_class = (GstElementClass *) klass;
  gstbasesrc_class = (GstBaseSrcClass *) klass;
  gstpushsrc_class = (GstPushSrcClass *) klass;

  GST_DEBUG_CATEGORY_INIT (enzui_curl_http_src_debug, "curlhttpsrc",
      0, "UriHandler for libcURL");

  gstelement_class->change_state =
      GST_DEBUG_FUNCPTR (enzui_curl_http_src_change_state);
  gstpushsrc_class->create = GST_DEBUG_FUNCPTR (enzui_curl_http_src_create);
  gstbasesrc_class->query = GST_DEBUG_FUNCPTR (enzui_curl_http_src_query);
  gstbasesrc_class->get_size = GST_DEBUG_FUNCPTR (enzui_curl_http_src_get_size);
  gstbasesrc_class->is_seekable =
      GST_DEBUG_FUNCPTR (enzui_curl_http_src_is_seekable);
  gstbasesrc_class->do_seek = GST_DEBUG_FUNCPTR (enzui_curl_http_src_do_seek);
  gstbasesrc_class->unlock = GST_DEBUG_FUNCPTR (enzui_curl_http_src_unlock);
  gstbasesrc_class->unlock_stop =
      GST_DEBUG_FUNCPTR (enzui_curl_http_src_unlock_stop);

  gst_element_class_add_pad_template (gstelement_class,
      gst_static_pad_template_get (&srcpadtemplate));

  enzui_curl_http_src_curl_capabilities = curl_version_info (CURLVERSION_NOW);
  gobject_class->set_property = enzui_curl_http_src_set_property;
  gobject_class->get_property = enzui_curl_http_src_get_property;
  gobject_class->finalize = enzui_curl_http_src_finalize;

  g_object_class_install_property (gobject_class, PROP_URI,
      g_param_spec_string ("location", "Location", "URI of resource to read",
          GSTCURL_HANDLE_DEFAULT_CURLOPT_URL,
          G_PARAM_READWRITE | G_PARAM_STATIC_STRINGS));

  g_object_class_install_property (gobject_class, PROP_USERNAME,
      g_param_spec_string ("user-id", "user-id",
          "HTTP location URI user id for authentication",
          GSTCURL_HANDLE_DEFAULT_CURLOPT_USERNAME,
          G_PARAM_READWRITE | G_PARAM_STATIC_STRINGS));

  g_object_class_install_property (gobject_class, PROP_PASSWORD,
      g_param_spec_string ("user-pw", "user-pw",
          "HTTP location URI password for authentication",
          GSTCURL_HANDLE_DEFAULT_CURLOPT_PASSWORD,
          G_PARAM_READWRITE | G_PARAM_STATIC_STRINGS));

  g_object_class_install_property (gobject_class, PROP_PROXYURI,
      g_param_spec_string ("proxy", "Proxy", "URI of HTTP proxy server",
          GSTCURL_HANDLE_DEFAULT_CURLOPT_PROXY,
          G_PARAM_READWRITE | G_PARAM_STATIC_STRINGS));

  g_object_class_install_property (gobject_class, PROP_PROXYUSERNAME,
      g_param_spec_string ("proxy-id", "proxy-id",
          "HTTP proxy URI user id for authentication",
          GSTCURL_HANDLE_DEFAULT_CURLOPT_PROXYUSERNAME,
          G_PARAM_READWRITE | G_PARAM_STATIC_STRINGS));

  g_object_class_install_property (gobject_class, PROP_PROXYPASSWORD,
      g_param_spec_string ("proxy-pw", "proxy-pw",
          "HTTP proxy URI password for authentication",
          GSTCURL_HANDLE_DEFAULT_CURLOPT_PROXYPASSWORD,
          G_PARAM_READWRITE | G_PARAM_STATIC_STRINGS));

  g_object_class_install_property (gobject_class, PROP_COOKIES,
      g_param_spec_boxed ("cookies", "Cookies", "List of HTTP Cookies",
          G_TYPE_STRV, G_PARAM_READWRITE | G_PARAM_STATIC_STRINGS));

  g_object_class_install_property (gobject_class, PROP_USERAGENT,
      g_param_spec_string ("user-agent", "User-Agent",
          "URI of resource requested",
          GSTCURL_HANDLE_DEFAULT_CURLOPT_USERAGENT "/<curl-version>",
          G_PARAM_READWRITE | G_PARAM_STATIC_STRINGS |
          GST_PARAM_DOC_SHOW_DEFAULT));

  g_object_class_install_property (gobject_class, PROP_COMPRESS,
      g_param_spec_boolean ("compress", "Compress",
          "Allow compressed content encodings",
          GSTCURL_HANDLE_DEFAULT_CURLOPT_ACCEPT_ENCODING,
          G_PARAM_READWRITE | G_PARAM_STATIC_STRINGS));

  g_object_class_install_property (gobject_class, PROP_REDIRECT,
      g_param_spec_boolean ("automatic-redirect", "automatic-redirect",
          "Allow HTTP Redirections (HTTP Status Code 300 series)",
          GSTCURL_HANDLE_DEFAULT_CURLOPT_FOLLOWLOCATION,
          G_PARAM_READWRITE | G_PARAM_STATIC_STRINGS));

  g_object_class_install_property (gobject_class, PROP_MAXREDIRECT,
      g_param_spec_int ("max-redirect", "Max-Redirect",
          "Maximum number of permitted redirections. -1 is unlimited.",
          GSTCURL_HANDLE_MIN_CURLOPT_MAXREDIRS,
          GSTCURL_HANDLE_MAX_CURLOPT_MAXREDIRS,
          GSTCURL_HANDLE_DEFAULT_CURLOPT_MAXREDIRS,
          G_PARAM_READWRITE | G_PARAM_STATIC_STRINGS));

  g_object_class_install_property (gobject_class, PROP_KEEPALIVE,
      g_param_spec_boolean ("keep-alive", "Keep-Alive",
          "Toggle keep-alive for connection reuse.",
          GSTCURL_HANDLE_DEFAULT_CURLOPT_TCP_KEEPALIVE,
          G_PARAM_READWRITE | G_PARAM_STATIC_STRINGS));

  g_object_class_install_property (gobject_class, PROP_TIMEOUT,
      g_param_spec_int ("timeout", "Timeout",
          "Value in seconds before timeout a blocking request (0 = no timeout)",
          GSTCURL_HANDLE_MIN_CURLOPT_TIMEOUT,
          GSTCURL_HANDLE_MAX_CURLOPT_TIMEOUT,
          GSTCURL_HANDLE_DEFAULT_CURLOPT_TIMEOUT,
          G_PARAM_READWRITE | G_PARAM_STATIC_STRINGS));

  g_object_class_install_property (gobject_class, PROP_HEADERS,
      g_param_spec_boxed ("extra-headers", "Extra Headers",
          "Extra headers to append to the HTTP request",
          GST_TYPE_STRUCTURE, G_PARAM_READWRITE | G_PARAM_STATIC_STRINGS));

  g_object_class_install_property (gobject_class, PROP_STRICT_SSL,
      g_param_spec_boolean ("ssl-strict", "SSL Strict",
          "Strict SSL certificate checking",
          GSTCURL_HANDLE_DEFAULT_CURLOPT_SSL_VERIFYPEER,
          G_PARAM_READWRITE | G_PARAM_STATIC_STRINGS));

  g_object_class_install_property (gobject_class, PROP_SSL_CA_FILE,
      g_param_spec_string ("ssl-ca-file", "SSL CA File",
          "Location of an SSL CA file to use for checking SSL certificates",
          GSTCURL_HANDLE_DEFAULT_CURLOPT_CAINFO,
          G_PARAM_READWRITE | G_PARAM_STATIC_STRINGS));

  g_object_class_install_property (gobject_class, PROP_RETRIES,
      g_param_spec_int ("retries", "Retries",
          "Maximum number of retries until giving up (-1=infinite)",
          GSTCURL_HANDLE_MIN_RETRIES, GSTCURL_HANDLE_MAX_RETRIES,
          GSTCURL_HANDLE_DEFAULT_RETRIES,
          G_PARAM_READWRITE | G_PARAM_STATIC_STRINGS));

  g_object_class_install_property (gobject_class, PROP_CONNECTIONMAXTIME,
      g_param_spec_uint ("max-connection-time", "Max-Connection-Time",
          "Maximum amount of time to keep-alive HTTP connections",
          GSTCURL_MIN_CONNECTION_TIME, GSTCURL_MAX_CONNECTION_TIME,
          GSTCURL_DEFAULT_CONNECTION_TIME,
          G_PARAM_READWRITE | G_PARAM_STATIC_STRINGS));

  g_object_class_install_property (gobject_class, PROP_MAXCONCURRENT_SERVER,
      g_param_spec_uint ("max-connections-per-server",
          "Max-Connections-Per-Server",
          "Maximum number of connections allowed per server for HTTP/1.x",
          GSTCURL_MIN_CONNECTIONS_SERVER, GSTCURL_MAX_CONNECTIONS_SERVER,
          GSTCURL_DEFAULT_CONNECTIONS_SERVER,
          G_PARAM_READWRITE | G_PARAM_STATIC_STRINGS));

  g_object_class_install_property (gobject_class, PROP_MAXCONCURRENT_PROXY,
      g_param_spec_uint ("max-connections-per-proxy",
          "Max-Connections-Per-Proxy",
          "Maximum number of concurrent connections allowed per proxy for HTTP/1.x",
          GSTCURL_MIN_CONNECTIONS_PROXY, GSTCURL_MAX_CONNECTIONS_PROXY,
          GSTCURL_DEFAULT_CONNECTIONS_PROXY,
          G_PARAM_READWRITE | G_PARAM_STATIC_STRINGS));

  g_object_class_install_property (gobject_class, PROP_MAXCONCURRENT_GLOBAL,
      g_param_spec_uint ("max-connections", "Max-Connections",
          "Maximum number of concurrent connections allowed for HTTP/1.x",
          GSTCURL_MIN_CONNECTIONS_GLOBAL, GSTCURL_MAX_CONNECTIONS_GLOBAL,
          GSTCURL_DEFAULT_CONNECTIONS_GLOBAL,
          G_PARAM_READWRITE | G_PARAM_STATIC_STRINGS));

  g_object_class_install_property (gobject_class, PROP_HTTPVERSION,
      g_param_spec_enum ("http-version", "HTTP-Version",
          "The preferred HTTP protocol version",
          GST_TYPE_CURL_HTTP_VERSION, GSTCURL_HTTP_VERSION_1_1,
          G_PARAM_READABLE | G_PARAM_STATIC_STRINGS));
  g_object_class_install_property (gobject_class, PROP_RECEIVE_LIMIT,
      g_param_spec_uint64 ("receive-buffer-limit", "Receive buffer limit",
          "Maximum native response bytes retained before pausing curl",
          0, G_MAXUINT64, ENZUI_RECEIVE_LIMIT, G_PARAM_READABLE | G_PARAM_STATIC_STRINGS));
  g_object_class_install_property (gobject_class, PROP_BUFFERED_BYTES,
      g_param_spec_uint64 ("buffered-bytes", "Buffered bytes", "Pending native response bytes",
          0, G_MAXUINT64, 0, G_PARAM_READABLE | G_PARAM_STATIC_STRINGS));
  g_object_class_install_property (gobject_class, PROP_PEAK_BUFFERED_BYTES,
      g_param_spec_uint64 ("peak-buffered-bytes", "Peak buffered bytes", "Largest retained native response body",
          0, G_MAXUINT64, 0, G_PARAM_READABLE | G_PARAM_STATIC_STRINGS));
  g_object_class_install_property (gobject_class, PROP_BACKPRESSURE_COUNT,
      g_param_spec_uint64 ("backpressure-count", "Backpressure count", "Curl receive pauses imposed by the buffer limit",
          0, G_MAXUINT64, 0, G_PARAM_READABLE | G_PARAM_STATIC_STRINGS));

  g_object_class_install_property (gobject_class, PROP_RECEIVE_PAUSED,
      g_param_spec_boolean ("receive-paused", "Receive paused", "Curl is paused at the native receive limit",
          FALSE, G_PARAM_READABLE | G_PARAM_STATIC_STRINGS));

  g_object_class_install_property (gobject_class, PROP_READ_AHEAD_CAPACITY,
      g_param_spec_uint ("read-ahead-capacity", "Read-ahead capacity",
          "Reserved anonymous file capacity, zero uses the bounded memory receiver",
          0, G_MAXUINT, 0, G_PARAM_READWRITE | G_PARAM_STATIC_STRINGS | GST_PARAM_MUTABLE_READY));
  g_object_class_install_property (gobject_class, PROP_READ_AHEAD_FD,
      g_param_spec_int ("read-ahead-fd", "Read-ahead descriptor",
          "Borrowed private anonymous read-write descriptor, duplicated on assignment",
          -1, G_MAXINT, -1, G_PARAM_WRITABLE | G_PARAM_STATIC_STRINGS | GST_PARAM_MUTABLE_READY));
  g_object_class_install_property (gobject_class, PROP_READ_AHEAD_READY,
      g_param_spec_boolean ("read-ahead-ready", "Read-ahead ready",
          "A private anonymous descriptor with the configured capacity is owned",
          FALSE, G_PARAM_READABLE | G_PARAM_STATIC_STRINGS));
  g_object_class_install_property (gobject_class, PROP_READ_AHEAD_BUFFERED,
      g_param_spec_uint64 ("read-ahead-buffered-bytes", "Read-ahead buffered bytes",
          "Unread response bytes retained in the anonymous FIFO",
          0, G_MAXUINT64, 0, G_PARAM_READABLE | G_PARAM_STATIC_STRINGS));
  g_object_class_install_property (gobject_class, PROP_READ_AHEAD_PEAK,
      g_param_spec_uint64 ("read-ahead-peak-bytes", "Read-ahead peak bytes",
          "Largest unread response retained in the anonymous FIFO",
          0, G_MAXUINT64, 0, G_PARAM_READABLE | G_PARAM_STATIC_STRINGS));
  g_object_class_install_property (gobject_class, PROP_READ_AHEAD_FILE_BYTES,
      g_param_spec_uint64 ("read-ahead-file-bytes", "Read-ahead file bytes",
          "Current logical size of the owned anonymous file",
          0, G_MAXUINT64, 0, G_PARAM_READABLE | G_PARAM_STATIC_STRINGS));

  /* Add a debugging task so it's easier to debug in the Multi worker thread */
  GST_DEBUG_CATEGORY_INIT (enzui_curl_loop_debug, "curl_multi_loop", 0,
      "libcURL loop thread debugging");
#ifndef GST_DISABLE_GST_DEBUG
  gst_debug_log (enzui_curl_loop_debug, GST_LEVEL_INFO, __FILE__, __func__,
      __LINE__, NULL, "Testing the curl_multi_loop debugging prints");
#endif



  gst_element_class_set_static_metadata (gstelement_class,
      "HTTP Client Source using libcURL",
      "Source/Network",
      "Receiver data as a client over a network via HTTP using cURL",
      "Sam Hurst <samuelh@rd.bbc.co.uk>");

}

static void
enzui_curl_http_src_reset (GstEnzCurlHttpSrc * source)
{
  g_mutex_lock (&source->buffer_mutex);

  source->transfer_begun = FALSE;
  source->data_received = FALSE;
  source->receive_paused = FALSE;
  source->resume_requested = FALSE;
  enzui_clear_body (source);
  source->retries_remaining = source->total_retries;
  source->content_size = 0;
  source->request_position = 0;
  source->stop_position = -1;
  if (source->http_headers != NULL) {
    gst_structure_free (source->http_headers);
    source->http_headers = NULL;
  }
  if (source->curl_handle) {
    enzui_curl_http_src_destroy_easy_handle (source);
    source->curl_handle = NULL;
  }

  g_mutex_unlock (&source->buffer_mutex);
}

static gboolean
enzui_curl_http_src_update_uri (GstEnzCurlHttpSrc * source, const gchar * uri)
{
  gboolean changed = FALSE;
  gboolean rc = TRUE;
  GSTCURL_FUNCTION_ENTRY (source);

  g_mutex_lock (&source->uri_mutex);
  if (g_strcmp0 (source->uri, uri) != 0) {
    if (source->uri) {
      GST_DEBUG_OBJECT (source,
          "URI already present as %s, updating to new URI %s", source->uri,
          uri);
      g_free (source->uri);
    }
    source->uri = g_strdup (uri);
    if (source->uri == NULL)
      rc = FALSE;
    changed = TRUE;
  }
  g_mutex_unlock (&source->uri_mutex);

  if (changed) {
    enzui_curl_http_src_wait_until_removed (source);
    enzui_curl_http_src_reset (source);
  }

  GSTCURL_FUNCTION_EXIT (source);
  return rc;
}

static void
enzui_curl_http_src_set_property (GObject * object, guint prop_id,
    const GValue * value, GParamSpec * pspec)
{
  GstEnzCurlHttpSrc *source = GST_CURLHTTPSRC (object);
  GSTCURL_FUNCTION_ENTRY (source);

  switch (prop_id) {
    case PROP_READ_AHEAD_CAPACITY:
    case PROP_READ_AHEAD_FD:
      g_mutex_lock (&source->buffer_mutex);
      if (!source->transfer_begun && GST_STATE (source) <= GST_STATE_READY) {
        if (prop_id == PROP_READ_AHEAD_CAPACITY) {
          enzui_close_read_ahead (source);
          enzui_clear_body (source);
          source->read_ahead_capacity = g_value_get_uint (value);
          source->read_ahead_peak = 0;
        } else {
          enzui_import_read_ahead (source, g_value_get_int (value));
        }
      }
      g_mutex_unlock (&source->buffer_mutex);
      break;
    case PROP_URI:
      enzui_curl_http_src_update_uri (source, g_value_get_string (value));
      break;
    case PROP_USERNAME:
      g_free (source->username);
      source->username = g_value_dup_string (value);
      break;
    case PROP_PASSWORD:
      g_free (source->password);
      source->password = g_value_dup_string (value);
      break;
    case PROP_PROXYURI:
      g_free (source->proxy_uri);
      source->proxy_uri = g_value_dup_string (value);
      break;
    case PROP_PROXYUSERNAME:
      g_free (source->proxy_user);
      source->proxy_user = g_value_dup_string (value);
      break;
    case PROP_PROXYPASSWORD:
      g_free (source->proxy_pass);
      source->proxy_pass = g_value_dup_string (value);
      break;
    case PROP_COOKIES:
      g_strfreev (source->cookies);
      source->cookies = g_strdupv (g_value_get_boxed (value));
      source->number_cookies = g_strv_length (source->cookies);
      break;
    case PROP_USERAGENT:
      g_free (source->user_agent);
      source->user_agent = g_value_dup_string (value);
      break;
    case PROP_HEADERS:
    {
      const GstStructure *s = gst_value_get_structure (value);
      if (source->request_headers)
        gst_structure_free (source->request_headers);
      source->request_headers =
          s ? gst_structure_copy (s) :
          gst_structure_new_empty (REQUEST_HEADERS_NAME);
    }
      break;
    case PROP_COMPRESS:
      source->accept_compressed_encodings = g_value_get_boolean (value);
      break;
    case PROP_REDIRECT:
      source->allow_3xx_redirect = g_value_get_boolean (value);
      break;
    case PROP_MAXREDIRECT:
      source->max_3xx_redirects = g_value_get_int (value);
      break;
    case PROP_KEEPALIVE:
      source->keep_alive = g_value_get_boolean (value);
      break;
    case PROP_TIMEOUT:
      source->timeout_secs = g_value_get_int (value);
      break;
    case PROP_STRICT_SSL:
      source->strict_ssl = g_value_get_boolean (value);
      break;
    case PROP_SSL_CA_FILE:
      g_free (source->custom_ca_file);
      source->custom_ca_file = g_value_dup_string (value);
      break;
    case PROP_RETRIES:
      source->total_retries = g_value_get_int (value);
      break;
    case PROP_CONNECTIONMAXTIME:
      source->max_connection_time = g_value_get_uint (value);
      break;
    case PROP_MAXCONCURRENT_SERVER:
      source->max_conns_per_server = g_value_get_uint (value);
      break;
    case PROP_MAXCONCURRENT_PROXY:
      source->max_conns_per_proxy = g_value_get_uint (value);
      break;
    case PROP_MAXCONCURRENT_GLOBAL:
      source->max_conns_global = g_value_get_uint (value);
      break;
    case PROP_HTTPVERSION:
      source->preferred_http_version = g_value_get_enum (value);
      break;
    default:
      G_OBJECT_WARN_INVALID_PROPERTY_ID (object, prop_id, pspec);
      break;
  }
  GSTCURL_FUNCTION_EXIT (source);
}

static void
enzui_curl_http_src_get_property (GObject * object, guint prop_id,
    GValue * value, GParamSpec * pspec)
{
  GstEnzCurlHttpSrc *source = GST_CURLHTTPSRC (object);
  GSTCURL_FUNCTION_ENTRY (source);

  switch (prop_id) {
    case PROP_URI:
      g_mutex_lock (&source->uri_mutex);
      g_value_set_string (value, source->uri);
      g_mutex_unlock (&source->uri_mutex);
      break;
    case PROP_USERNAME:
      g_value_set_string (value, source->username);
      break;
    case PROP_PASSWORD:
      g_value_set_string (value, source->password);
      break;
    case PROP_PROXYURI:
      g_value_set_string (value, source->proxy_uri);
      break;
    case PROP_PROXYUSERNAME:
      g_value_set_string (value, source->proxy_user);
      break;
    case PROP_PROXYPASSWORD:
      g_value_set_string (value, source->proxy_pass);
      break;
    case PROP_COOKIES:
      g_value_set_boxed (value, source->cookies);
      break;
    case PROP_USERAGENT:
      g_value_set_string (value, source->user_agent);
      break;
    case PROP_HEADERS:
      gst_value_set_structure (value, source->request_headers);
      break;
    case PROP_COMPRESS:
      g_value_set_boolean (value, source->accept_compressed_encodings);
      break;
    case PROP_REDIRECT:
      g_value_set_boolean (value, source->allow_3xx_redirect);
      break;
    case PROP_MAXREDIRECT:
      g_value_set_int (value, source->max_3xx_redirects);
      break;
    case PROP_KEEPALIVE:
      g_value_set_boolean (value, source->keep_alive);
      break;
    case PROP_TIMEOUT:
      g_value_set_int (value, source->timeout_secs);
      break;
    case PROP_STRICT_SSL:
      g_value_set_boolean (value, source->strict_ssl);
      break;
    case PROP_SSL_CA_FILE:
      g_value_set_string (value, source->custom_ca_file);
      break;
    case PROP_RETRIES:
      g_value_set_int (value, source->total_retries);
      break;
    case PROP_CONNECTIONMAXTIME:
      g_value_set_uint (value, source->max_connection_time);
      break;
    case PROP_MAXCONCURRENT_SERVER:
      g_value_set_uint (value, source->max_conns_per_server);
      break;
    case PROP_MAXCONCURRENT_PROXY:
      g_value_set_uint (value, source->max_conns_per_proxy);
      break;
    case PROP_MAXCONCURRENT_GLOBAL:
      g_value_set_uint (value, source->max_conns_global);
      break;
    case PROP_HTTPVERSION:
      g_value_set_enum (value, source->preferred_http_version);
      break;
    case PROP_READ_AHEAD_CAPACITY:
      g_mutex_lock (&source->buffer_mutex);
      g_value_set_uint (value, source->read_ahead_capacity);
      g_mutex_unlock (&source->buffer_mutex);
      break;
    case PROP_READ_AHEAD_READY:
      g_mutex_lock (&source->buffer_mutex);
      g_value_set_boolean (value, enzui_read_ahead_enabled (source));
      g_mutex_unlock (&source->buffer_mutex);
      break;
    case PROP_READ_AHEAD_BUFFERED:
    case PROP_READ_AHEAD_PEAK:
    case PROP_READ_AHEAD_FILE_BYTES:
      g_mutex_lock (&source->buffer_mutex);
      if (prop_id == PROP_READ_AHEAD_BUFFERED) {
        g_value_set_uint64 (value, enzui_read_ahead_enabled (source) ? source->buffer_len : 0);
      } else if (prop_id == PROP_READ_AHEAD_PEAK) {
        g_value_set_uint64 (value, source->read_ahead_peak);
      } else {
        guint64 length = 0;
#ifdef __linux__
        struct stat metadata;
        if (source->read_ahead_fd >= 0 && fstat (source->read_ahead_fd, &metadata) == 0)
          length = (guint64) metadata.st_size;
#endif
        g_value_set_uint64 (value, length);
      }
      g_mutex_unlock (&source->buffer_mutex);
      break;
    case PROP_RECEIVE_LIMIT:
      g_value_set_uint64 (value, ENZUI_RECEIVE_LIMIT);
      break;
    case PROP_RECEIVE_PAUSED:
      g_mutex_lock (&source->buffer_mutex);
      g_value_set_boolean (value, source->receive_paused);
      g_mutex_unlock (&source->buffer_mutex);
      break;
    case PROP_BUFFERED_BYTES:
    case PROP_PEAK_BUFFERED_BYTES:
    case PROP_BACKPRESSURE_COUNT:
      g_mutex_lock (&source->buffer_mutex);
      g_value_set_uint64 (value, prop_id == PROP_BUFFERED_BYTES ? (enzui_read_ahead_enabled (source) ? 0 : source->buffer_len) :
          prop_id == PROP_PEAK_BUFFERED_BYTES ? source->peak_buffered_bytes : source->backpressure_count);
      g_mutex_unlock (&source->buffer_mutex);
      break;
    default:
      G_OBJECT_WARN_INVALID_PROPERTY_ID (object, prop_id, pspec);
      break;
  }
  GSTCURL_FUNCTION_EXIT (source);
}

static void
enzui_curl_http_src_init (GstEnzCurlHttpSrc * source)
{
  GSTCURL_FUNCTION_ENTRY (source);

  /* Assume everything is already free'd */
  source->uri = NULL;
  source->redirect_uri = NULL;
  source->username = GSTCURL_HANDLE_DEFAULT_CURLOPT_USERNAME;
  source->password = GSTCURL_HANDLE_DEFAULT_CURLOPT_PASSWORD;
  source->proxy_uri = NULL;
  source->proxy_user = NULL;
  source->proxy_pass = NULL;
  source->cookies = NULL;
  g_assert (enzui_curl_http_src_curl_capabilities != NULL);
  source->user_agent =
      g_strdup_printf (GSTCURL_HANDLE_DEFAULT_CURLOPT_USERAGENT "/%s",
      enzui_curl_http_src_curl_capabilities->version);
  source->number_cookies = 0;
  source->request_headers = gst_structure_new_empty (REQUEST_HEADERS_NAME);
  source->allow_3xx_redirect = GSTCURL_HANDLE_DEFAULT_CURLOPT_FOLLOWLOCATION;
  source->max_3xx_redirects = GSTCURL_HANDLE_DEFAULT_CURLOPT_MAXREDIRS;
  source->keep_alive = GSTCURL_HANDLE_DEFAULT_CURLOPT_TCP_KEEPALIVE;
  source->timeout_secs = GSTCURL_HANDLE_DEFAULT_CURLOPT_TIMEOUT;
  source->max_connection_time = GSTCURL_DEFAULT_CONNECTION_TIME;
  source->max_conns_per_server = GSTCURL_DEFAULT_CONNECTIONS_SERVER;
  source->max_conns_per_proxy = GSTCURL_DEFAULT_CONNECTIONS_PROXY;
  source->max_conns_global = GSTCURL_DEFAULT_CONNECTIONS_GLOBAL;
  source->strict_ssl = GSTCURL_HANDLE_DEFAULT_CURLOPT_SSL_VERIFYPEER;
  source->custom_ca_file = NULL;
  source->preferred_http_version = GSTCURL_HTTP_VERSION_1_1;
  source->total_retries = GSTCURL_HANDLE_DEFAULT_RETRIES;
  source->retries_remaining = source->total_retries;
  source->slist = NULL;
  source->accept_compressed_encodings = FALSE;
  source->seekable = GSTCURL_SEEKABLE_UNKNOWN;
  source->content_size = 0;
  source->request_position = 0;
  source->stop_position = -1;

  gst_base_src_set_automatic_eos (GST_BASE_SRC (source), FALSE);

  source->proxy_uri = g_strdup (g_getenv ("http_proxy"));
  source->no_proxy_list = g_strdup ("");

  g_mutex_init (&source->uri_mutex);
  g_mutex_init (&source->buffer_mutex);
  g_cond_init (&source->buffer_cond);

  source->buffer = NULL;
  source->buffer_len = 0;
  source->read_ahead_fd = -1;
  source->read_ahead_capacity = 0;
  source->read_ahead_read = 0;
  source->read_ahead_write = 0;
  source->read_ahead_peak = 0;
  source->receive_paused = FALSE;
  source->resume_requested = FALSE;
  source->peak_buffered_bytes = 0;
  source->backpressure_count = 0;
  source->multi_task_context.state = GSTCURL_MULTI_LOOP_STATE_STOP;
  g_mutex_init (&source->multi_task_context.mutex);
  g_cond_init (&source->multi_task_context.signal);
  source->state = GSTCURL_NONE;
  source->pending_state = GSTCURL_NONE;
  source->transfer_begun = FALSE;
  source->data_received = FALSE;
  source->connection_status = GSTCURL_NOT_CONNECTED;

  source->http_headers = NULL;
  source->status_code = 0;
  source->reason_phrase = NULL;
  source->hdrs_updated = FALSE;
  source->curl_result = CURLE_OK;
  gst_caps_replace (&source->caps, NULL);

  GSTCURL_FUNCTION_EXIT (source);
}

/*
 * Check if the Curl multi loop has been started. If not, initialise it and
 * start it running. If it is already running, increment the refcount.
 */
static gboolean
enzui_curl_http_src_ref_multi (GstEnzCurlHttpSrc * src)
{

  GSTCURL_FUNCTION_ENTRY (src);


  g_mutex_lock (&src->multi_task_context.mutex);
  if (src->multi_task_context.refcount == 0) {
    /* Set up various in-task properties */

    /* NULL is treated as the start of the list, no need to allocate. */
    src->multi_task_context.queue = NULL;

    /* set up curl */
    src->multi_task_context.multi_handle = curl_multi_init ();

    curl_multi_setopt (src->multi_task_context.multi_handle,
        CURLMOPT_PIPELINING, 1L);
#ifdef CURLMOPT_MAX_HOST_CONNECTIONS
    curl_multi_setopt (src->multi_task_context.multi_handle,
        CURLMOPT_MAX_HOST_CONNECTIONS, 1L);
#endif

    /* Start the thread */
    g_rec_mutex_init (&src->multi_task_context.task_rec_mutex);
    src->multi_task_context.state = GSTCURL_MULTI_LOOP_STATE_RUNNING;
    src->multi_task_context.task = gst_task_new (
        (GstTaskFunction) enzui_curl_http_src_curl_multi_loop,
        (gpointer) & src->multi_task_context, NULL);
    gst_task_set_lock (src->multi_task_context.task,
        &src->multi_task_context.task_rec_mutex);
    if (gst_task_start (src->multi_task_context.task) == FALSE) {
      src->multi_task_context.refcount = 1;
      g_mutex_unlock (&src->multi_task_context.mutex);
      enzui_curl_http_src_unref_multi (src);
      return FALSE;
    }
    GSTCURL_INFO_PRINT ("Curl multi loop has been correctly initialised!");
  }
  src->multi_task_context.refcount++;
  g_mutex_unlock (&src->multi_task_context.mutex);

  GSTCURL_FUNCTION_EXIT (src);
  return TRUE;
}

static void
enzui_curl_http_src_unref_multi (GstEnzCurlHttpSrc * src)
{

  GSTCURL_FUNCTION_ENTRY (src);


  g_mutex_lock (&src->multi_task_context.mutex);
  src->multi_task_context.refcount--;
  GST_INFO_OBJECT (src, "Closing instance, worker thread refcount is now %u",
      src->multi_task_context.refcount);

  if (src->multi_task_context.refcount == 0) {
    /* Everything's done! Clean up. */
    gst_task_stop (src->multi_task_context.task);
    src->multi_task_context.state = GSTCURL_MULTI_LOOP_STATE_STOP;
    g_cond_signal (&src->multi_task_context.signal);
    if (src->multi_task_context.multi_handle != NULL)
      curl_multi_wakeup (src->multi_task_context.multi_handle);
    g_mutex_unlock (&src->multi_task_context.mutex);
    GST_DEBUG_OBJECT (src, "Joining curl_multi_loop task...");
    gst_task_join (src->multi_task_context.task);
    gst_object_unref (src->multi_task_context.task);
    src->multi_task_context.task = NULL;
    curl_multi_cleanup (src->multi_task_context.multi_handle);
    src->multi_task_context.multi_handle = NULL;
    g_rec_mutex_clear (&src->multi_task_context.task_rec_mutex);
    GST_DEBUG_OBJECT (src, "multi_task_context cleanup complete");
  } else {
    g_mutex_unlock (&src->multi_task_context.mutex);
  }

  GSTCURL_FUNCTION_EXIT (src);
}

static void
enzui_curl_http_src_finalize (GObject * obj)
{
  GstEnzCurlHttpSrc *src = GST_CURLHTTPSRC (obj);

  GSTCURL_FUNCTION_ENTRY (src);

  /* Cleanup all memory allocated */
  enzui_curl_http_src_cleanup_instance (src);
  enzui_close_read_ahead (src);

  GSTCURL_FUNCTION_EXIT (src);

  /* Chain up to parent class */
  G_OBJECT_CLASS (enzui_curl_http_src_parent_class)->finalize (obj);
}

/*
 * Do the transfer. If the transfer hasn't begun yet, start a new curl handle
 * and pass it to the multi queue to be operated on. Then wait for any blocks
 * of data and push them to the source pad.
 */
static GstFlowReturn
enzui_curl_http_src_create (GstPushSrc * psrc, GstBuffer ** outbuf)
{
  GstFlowReturn ret;
  GstEnzCurlHttpSrc *src = GST_CURLHTTPSRC (psrc);
  GstStructure *empty_headers;
  GstBaseSrc *basesrc;
  gboolean wake_multi = FALSE;

  GSTCURL_FUNCTION_ENTRY (src);

  basesrc = GST_BASE_SRC_CAST (src);

retry:
  ret = GST_FLOW_OK;
  /* NOTE: when both the buffer_mutex and multi_task_context.mutex are
     needed, multi_task_context.mutex must be acquired first */
  g_mutex_lock (&src->multi_task_context.mutex);
  g_mutex_lock (&src->buffer_mutex);
  if (src->state == GSTCURL_UNLOCK) {
    ret = GST_FLOW_FLUSHING;
    goto escape;
  }

  if ((src->read_ahead_capacity != 0) != enzui_read_ahead_enabled (src)) {
    g_mutex_unlock (&src->buffer_mutex);
    g_mutex_unlock (&src->multi_task_context.mutex);
    GST_ELEMENT_ERROR (src, RESOURCE, SETTINGS,
        ("The anonymous read-ahead buffer is not configured"), (NULL));
    return GST_FLOW_ERROR;
  }

  if (!src->transfer_begun) {
    GST_DEBUG_OBJECT (src, "Starting new request for URI %s", src->uri);
    /* A replacement request must not inherit a paused handle or its pending callback. */
    enzui_curl_http_src_destroy_easy_handle (src);
    src->receive_paused = FALSE;
    src->resume_requested = FALSE;
    src->curl_handle = enzui_curl_http_src_create_easy_handle (src);
    if (src->curl_handle == NULL) {
      ret = GST_FLOW_ERROR;
      goto escape;
    }

    if (enzui_curl_http_src_add_queue_item (&src->multi_task_context.queue, src)
        == FALSE) {
      GST_ERROR_OBJECT (src, "Couldn't create new queue item! Aborting...");
      ret = GST_FLOW_ERROR;
      goto escape;
    }
    /* Signal the worker thread */
    g_cond_signal (&src->multi_task_context.signal);
    if (src->multi_task_context.multi_handle != NULL)
      curl_multi_wakeup (src->multi_task_context.multi_handle);

    src->state = GSTCURL_OK;
    src->transfer_begun = TRUE;
    src->data_received = FALSE;

    GST_DEBUG_OBJECT (src, "Submitted request for URI %s to curl", src->uri);

    if (src->http_headers != NULL) {
      gst_structure_free (src->http_headers);
    }
    empty_headers = gst_structure_new_empty (RESPONSE_HEADERS_NAME);
    src->http_headers = gst_structure_new (HTTP_HEADERS_NAME,
        URI_NAME, G_TYPE_STRING, src->uri,
        REQUEST_HEADERS_NAME, GST_TYPE_STRUCTURE, src->request_headers,
        RESPONSE_HEADERS_NAME, GST_TYPE_STRUCTURE, empty_headers, NULL);
    gst_structure_free (empty_headers);
    GST_INFO_OBJECT (src, "Created a new headers object");
  }

  g_mutex_unlock (&src->multi_task_context.mutex);

  /* Wait for data to become available, then punt it downstream */
  while ((src->buffer_len == 0) && (src->state == GSTCURL_OK)
      && (src->connection_status == GSTCURL_CONNECTED)) {
    g_cond_wait (&src->buffer_cond, &src->buffer_mutex);
  }

  if (src->state == GSTCURL_UNLOCK) {
    enzui_clear_body (src);
    g_mutex_unlock (&src->buffer_mutex);
    return GST_FLOW_FLUSHING;
  }

  ret = enzui_curl_http_src_handle_response (src);
  switch (ret) {
    case GST_FLOW_ERROR:
      /* Don't attempt a retry, just bomb out */
      g_mutex_unlock (&src->buffer_mutex);
      return ret;
    case GST_FLOW_CUSTOM_ERROR:
      if (src->data_received == TRUE) {
        /*
         * If data has already been received, we can't recall previously sent
         * buffers so don't attempt a retry in this case.
         *
         * TODO: Remember the position we got to, and make a range request for
         * the resource without the bit we've already received?
         */
        GST_WARNING_OBJECT (src,
            "Failed mid-transfer, can't continue for URI %s", src->uri);
        g_mutex_unlock (&src->buffer_mutex);
        return GST_FLOW_ERROR;
      }
      if (src->retries_remaining <= 0 || --src->retries_remaining == 0) {
        GST_WARNING_OBJECT (src, "Out of retries for URI %s", src->uri);
        g_mutex_unlock (&src->buffer_mutex);
        return GST_FLOW_ERROR;  /* Don't attempt a retry, just bomb out */
      }
      GST_INFO_OBJECT (src, "Attempting retry for URI %s", src->uri);
      src->state = GSTCURL_NONE;
      src->transfer_begun = FALSE;
      src->status_code = 0;
      g_free (src->reason_phrase);
      src->reason_phrase = NULL;
      src->hdrs_updated = FALSE;
      if (src->http_headers != NULL) {
        gst_structure_free (src->http_headers);
        src->http_headers = NULL;
        GST_INFO_OBJECT (src, "NULL'd the headers");
      }
      enzui_curl_http_src_destroy_easy_handle (src);
      g_mutex_unlock (&src->buffer_mutex);
      goto retry;               /* Attempt a retry! */
    default:
      break;
  }

  if (((src->state == GSTCURL_OK) || (src->state == GSTCURL_DONE)) &&
      (src->buffer_len > 0)) {

    GST_DEBUG_OBJECT (src, "Pushing %u bytes of transfer for URI %s to pad",
        src->buffer_len, src->uri);
    guint length = MIN (src->buffer_len, ENZUI_RECEIVE_LIMIT);
    *outbuf = gst_buffer_new_allocate (NULL, length, NULL);
    if (*outbuf == NULL) {
      g_mutex_unlock (&src->buffer_mutex);
      return GST_FLOW_ERROR;
    }
    if (enzui_read_ahead_enabled (src)) {
      GstMapInfo mapped;
      if (!gst_buffer_map (*outbuf, &mapped, GST_MAP_WRITE)) {
        gst_buffer_unref (*outbuf);
        *outbuf = NULL;
        g_mutex_unlock (&src->buffer_mutex);
        return GST_FLOW_ERROR;
      }
      gboolean read_ok = enzui_read_ahead_io (src, src->read_ahead_read, length,
          (gchar *) mapped.data, FALSE);
      gst_buffer_unmap (*outbuf, &mapped);
      if (!read_ok) {
        gst_buffer_unref (*outbuf);
        *outbuf = NULL;
        g_mutex_unlock (&src->buffer_mutex);
        GST_ELEMENT_ERROR (src, RESOURCE, READ,
            ("The anonymous read-ahead buffer could not be read"), (NULL));
        return GST_FLOW_ERROR;
      }
      src->read_ahead_read = (guint) (((guint64) src->read_ahead_read + length) % src->read_ahead_capacity);
      src->buffer_len -= length;
    } else {
      gst_buffer_fill (*outbuf, 0, src->buffer, length);
      enzui_clear_body (src);
    }
    GST_BUFFER_OFFSET (*outbuf) = basesrc->segment.position;
    src->data_received = TRUE;
    if (src->receive_paused) {
      src->resume_requested = TRUE;
      wake_multi = TRUE;
    }

    /* ret should still be GST_FLOW_OK */
  } else if ((src->state == GSTCURL_DONE) && (src->buffer_len == 0)) {
    GST_INFO_OBJECT (src, "Full body received, signalling EOS for URI %s.",
        src->uri);
    src->state = GSTCURL_NONE;
    src->transfer_begun = FALSE;
    src->status_code = 0;
    g_free (src->reason_phrase);
    src->reason_phrase = NULL;
    src->hdrs_updated = FALSE;
    enzui_curl_http_src_destroy_easy_handle (src);
    ret = GST_FLOW_EOS;
  } else {
    switch (src->state) {
      case GSTCURL_NONE:
        GST_WARNING_OBJECT (src, "Got unexpected GSTCURL_NONE state!");
        break;
      case GSTCURL_REMOVED:
        GST_WARNING_OBJECT (src, "Transfer got removed from the curl queue");
        ret = GST_FLOW_EOS;
        break;
      case GSTCURL_BAD_QUEUE_REQUEST:
        GST_ERROR_OBJECT (src, "Bad Queue Request!");
        ret = GST_FLOW_ERROR;
        break;
      case GSTCURL_TOTAL_ERROR:
        GST_ERROR_OBJECT (src, "Critical, unrecoverable error!");
        ret = GST_FLOW_ERROR;
        break;
      case GSTCURL_PIPELINE_NULL:
        GST_ERROR_OBJECT (src, "Pipeline null");
        break;
      default:
        GST_ERROR_OBJECT (src, "Unknown state of %u", src->state);
    }
  }
  g_mutex_unlock (&src->buffer_mutex);
  if (wake_multi)
    if (src->multi_task_context.multi_handle != NULL)
      curl_multi_wakeup (src->multi_task_context.multi_handle);
  GSTCURL_FUNCTION_EXIT (src);
  return ret;

escape:
  g_mutex_unlock (&src->buffer_mutex);
  g_mutex_unlock (&src->multi_task_context.mutex);

  GSTCURL_FUNCTION_EXIT (src);
  return ret;
}

/*
 * Convert header from a GstStructure type to a curl_slist type that curl will
 * understand.
 */
static gboolean
_headers_to_curl_slist (GQuark fieldname, const GValue * value,
    gpointer ptr)
{
  gchar *field;
  struct curl_slist **p_slist = ptr;

  field = g_strdup_printf ("%s: %s", g_quark_to_string (fieldname),
      g_value_get_string (value));

  *p_slist = curl_slist_append (*p_slist, field);

  g_free (field);

  return TRUE;
}

/*
 * From the data in the queue element s, create a CURL easy handle and populate
 * options with the URL, proxy data, login options, cookies,
 */
static CURL *
enzui_curl_http_src_create_easy_handle (GstEnzCurlHttpSrc * s)
{
  CURL *handle;
  gint i;
  GSTCURL_FUNCTION_ENTRY (s);

  /* This is mandatory and yet not default option, so if this is NULL
   * then something very bad is going on. */
  if (s->uri == NULL) {
    GST_ERROR_OBJECT (s, "No URI for curl!");
    return NULL;
  }

  handle = curl_easy_init ();
  if (handle == NULL) {
    GST_ERROR_OBJECT (s, "Couldn't init a curl easy handle!");
    return NULL;
  }
  GST_INFO_OBJECT (s, "Creating a new handle for URI %s", s->uri);



  enzui_curl_setopt_str (s, handle, CURLOPT_URL, s->uri);
  enzui_curl_setopt_str (s, handle, CURLOPT_USERNAME, s->username);
  enzui_curl_setopt_str (s, handle, CURLOPT_PASSWORD, s->password);
  enzui_curl_setopt_str (s, handle, CURLOPT_PROXY, s->proxy_uri);
  enzui_curl_setopt_str (s, handle, CURLOPT_NOPROXY, s->no_proxy_list);
  enzui_curl_setopt_str (s, handle, CURLOPT_PROXYUSERNAME, s->proxy_user);
  enzui_curl_setopt_str (s, handle, CURLOPT_PROXYPASSWORD, s->proxy_pass);
  enzui_curl_setopt_generic (s, handle, CURLOPT_HTTPAUTH, CURLAUTH_ANY);

  for (i = 0; i < s->number_cookies; i++) {
    enzui_curl_setopt_str (s, handle, CURLOPT_COOKIELIST, s->cookies[i]);
  }

  /* curl_slist_append dynamically allocates memory, but I need to free it */
  if (s->request_headers != NULL) {
    gst_structure_foreach (s->request_headers, _headers_to_curl_slist,
        &s->slist);
    if (curl_easy_setopt (handle, CURLOPT_HTTPHEADER, s->slist) != CURLE_OK) {
      GST_WARNING_OBJECT (s, "Failed to set HTTP headers!");
    }
  }

  enzui_curl_setopt_str_default (s, handle, CURLOPT_USERAGENT, s->user_agent);

  /*
   * Unlike soup, this isn't a binary op, curl wants a string here. So if it's
   * TRUE, simply set the value as an empty string as this allows both gzip and
   * zlib compression methods.
   */
  if (s->accept_compressed_encodings == TRUE) {
    enzui_curl_setopt_str (s, handle, CURLOPT_ACCEPT_ENCODING, "");
  } else {
    enzui_curl_setopt_str (s, handle, CURLOPT_ACCEPT_ENCODING, "identity");
  }

  enzui_curl_setopt_int (s, handle, CURLOPT_FOLLOWLOCATION,
      s->allow_3xx_redirect);
  enzui_curl_setopt_int_default (s, handle, CURLOPT_MAXREDIRS,
      s->max_3xx_redirects);
  enzui_curl_setopt_bool (s, handle, CURLOPT_TCP_KEEPALIVE, s->keep_alive);
  enzui_curl_setopt_int (s, handle, CURLOPT_TIMEOUT, (long) s->timeout_secs);
  enzui_curl_setopt_bool (s, handle, CURLOPT_SSL_VERIFYPEER, s->strict_ssl);
  enzui_curl_setopt_str (s, handle, CURLOPT_CAINFO, s->custom_ca_file);

  if (s->request_position || s->stop_position > 0) {
    gchar *range;
    if (s->stop_position < 1) {
      /* start specified, no end specified */
      range = g_strdup_printf ("%" G_GINT64_FORMAT "-", s->request_position);
    } else {
      /* in GStreamer the end position indicates the first byte that is not
         in the range, whereas in HTTP the Content-Range header includes the
         byte listed in the end value */
      range = g_strdup_printf ("%" G_GINT64_FORMAT "-%" G_GINT64_FORMAT,
          s->request_position, s->stop_position - 1);
    }
    GST_TRACE_OBJECT (s, "Requesting range: %s", range);
    curl_easy_setopt (handle, CURLOPT_RANGE, range);
    g_free (range);
  }

  switch (s->preferred_http_version) {
    case GSTCURL_HTTP_VERSION_1_0:
      GST_DEBUG_OBJECT (s, "Setting version as HTTP/1.0");
      enzui_curl_setopt_int (s, handle, CURLOPT_HTTP_VERSION,
          CURL_HTTP_VERSION_1_0);
      break;
    case GSTCURL_HTTP_VERSION_1_1:
      GST_DEBUG_OBJECT (s, "Setting version as HTTP/1.1");
      enzui_curl_setopt_int (s, handle, CURLOPT_HTTP_VERSION,
          CURL_HTTP_VERSION_1_1);
      break;
#ifdef CURL_VERSION_HTTP2
    case GSTCURL_HTTP_VERSION_2_0:
      GST_DEBUG_OBJECT (s, "Setting version as HTTP/2.0");
      if (curl_easy_setopt (handle, CURLOPT_HTTP_VERSION,
              CURL_HTTP_VERSION_2_0) != CURLE_OK) {
        if (enzui_curl_http_src_curl_capabilities->features & CURL_VERSION_HTTP2) {
          GST_WARNING_OBJECT (s,
              "Cannot set unsupported option CURLOPT_HTTP_VERSION");
        } else {
          GST_INFO_OBJECT (s, "HTTP/2 unsupported by libcurl at this time");
        }
      }
      break;
#endif
    default:
      GST_WARNING_OBJECT (s,
          "Supplied a bogus HTTP version, using curl default!");
  }

  enzui_curl_setopt_generic (s, handle, CURLOPT_HEADERFUNCTION,
      enzui_curl_http_src_get_header);
  enzui_curl_setopt_str (s, handle, CURLOPT_HEADERDATA, s);
  enzui_curl_setopt_generic (s, handle, CURLOPT_WRITEFUNCTION,
      enzui_curl_http_src_get_chunks);
  enzui_curl_setopt_str (s, handle, CURLOPT_WRITEDATA, s);

  enzui_curl_setopt_str (s, handle, CURLOPT_ERRORBUFFER, s->curl_errbuf);

  GSTCURL_FUNCTION_EXIT (s);
  return handle;
}

/*
 * Check the return type from the curl transfer. If it was okay, then deal with
 * any headers that were received. Headers should only be dealt with once - but
 * we might get a second set if there are trailing headers (RFC7230 Section 4.4)
 */
static GstFlowReturn
enzui_curl_http_src_handle_response (GstEnzCurlHttpSrc * src)
{
  glong curl_info_long;
  gdouble curl_info_dbl;
  gchar *redirect_url;
  const GValue *response_headers;
  GstFlowReturn ret = GST_FLOW_OK;

  GSTCURL_FUNCTION_ENTRY (src);

  GST_TRACE_OBJECT (src, "status code: %d (%s), curl return code %d",
      src->status_code, src->reason_phrase, src->curl_result);

  /* Check the curl result code first - anything not 0 is probably a failure */
  if (src->curl_result != 0) {
    GST_WARNING_OBJECT (src, "Curl failed the transfer (%d): %s",
        src->curl_result, curl_easy_strerror (src->curl_result));
    GST_DEBUG_OBJECT (src, "Reason for curl failure: %s", src->curl_errbuf);
    return GST_FLOW_ERROR;
  }

  /*
   * What response code do we have?
   */
  if (src->status_code >= 400) {
    GST_WARNING_OBJECT (src, "Transfer for URI %s returned error status %u",
        src->uri, src->status_code);
    src->retries_remaining = 0;
    CURL_HTTP_SRC_ERROR (src, RESOURCE, NOT_FOUND, (src->reason_phrase));
    return GST_FLOW_ERROR;
  } else if (src->status_code == 0) {
    if (curl_easy_getinfo (src->curl_handle, CURLINFO_TOTAL_TIME,
            &curl_info_dbl) != CURLE_OK) {
      /* Curl cannot be relied on in this state, so return an error. */
      return GST_FLOW_ERROR;
    }
    if (curl_info_dbl > src->timeout_secs) {
      return GST_FLOW_CUSTOM_ERROR;
    }

    if (curl_easy_getinfo (src->curl_handle, CURLINFO_OS_ERRNO,
            &curl_info_long) != CURLE_OK) {
      /* Curl cannot be relied on in this state, so return an error. */
      return GST_FLOW_ERROR;

    }

    GST_WARNING_OBJECT (src, "Errno for CONNECT call was %ld (%s)",
        curl_info_long, g_strerror ((gint) curl_info_long));

    /* Some of these responses are retry-able, others not. Set the returned
     * state to ERROR so we crash out instead of fruitlessly retrying.
     */
    if (curl_info_long == ECONNREFUSED) {
      return GST_FLOW_ERROR;
    }
    ret = GST_FLOW_CUSTOM_ERROR;
  }


  if (ret == GST_FLOW_CUSTOM_ERROR) {
    src->hdrs_updated = FALSE;
    GSTCURL_FUNCTION_EXIT (src);
    return ret;
  }

  /* Only do this once */
  if (src->hdrs_updated == FALSE) {
    GSTCURL_FUNCTION_EXIT (src);
    return GST_FLOW_OK;
  }

  /*
   * Deal with redirections...
   */
  if (curl_easy_getinfo (src->curl_handle, CURLINFO_EFFECTIVE_URL,
          &redirect_url)
      == CURLE_OK) {
    size_t lena, lenb;
    lena = strlen (src->uri);
    lenb = strlen (redirect_url);
    if (g_ascii_strncasecmp (src->uri, redirect_url,
            (lena > lenb) ? lenb : lena) != 0) {
      GST_INFO_OBJECT (src, "Got a redirect to %s, setting as redirect URI",
          redirect_url);
      src->redirect_uri = g_strdup (redirect_url);
      gst_structure_remove_field (src->http_headers, REDIRECT_URI_NAME);
      gst_structure_set (src->http_headers, REDIRECT_URI_NAME,
          G_TYPE_STRING, redirect_url, NULL);
    }
  }

  /*
   * Push the content length
   */
  src->content_size = enzui_curl_http_src_get_length_from_headers (src);
  if (src->content_size) {
    GstBaseSrc *basesrc = GST_BASE_SRC_CAST (src);

    if (src->seekable == GSTCURL_SEEKABLE_UNKNOWN) {
      src->seekable = GSTCURL_SEEKABLE_TRUE;
    }
    if (basesrc->segment.duration != src->content_size) {
      basesrc->segment.duration = src->content_size;
      gst_element_post_message (GST_ELEMENT (src),
          gst_message_new_duration_changed (GST_OBJECT (src)));
    }
  } else {
    GST_WARNING_OBJECT (src,
        "No Content-Length was specified in the response.");
    src->seekable = GSTCURL_SEEKABLE_FALSE;
  }

  /*
   * Push all the received headers down via a sicky event
   */
  response_headers = gst_structure_get_value (src->http_headers,
      RESPONSE_HEADERS_NAME);
  if (gst_structure_n_fields (gst_value_get_structure (response_headers)) > 0) {
    GstEvent *hdrs_event;

    gst_element_post_message (GST_ELEMENT_CAST (src),
        gst_message_new_element (GST_OBJECT_CAST (src),
            gst_structure_copy (src->http_headers)));

    /* gst_event_new_custom takes ownership of our structure */
    hdrs_event = gst_event_new_custom (GST_EVENT_CUSTOM_DOWNSTREAM_STICKY,
        gst_structure_copy (src->http_headers));
    gst_pad_push_event (GST_BASE_SRC_PAD (src), hdrs_event);
    GST_INFO_OBJECT (src, "Pushed headers downstream");
  }

  src->hdrs_updated = FALSE;

  GSTCURL_FUNCTION_EXIT (src);

  return ret;
}

/*
 * "Negotiate" capabilities between us and the sink.
 * I.e. tell the sink device what data to expect. We can't be told what to send
 * unless we implement "only return to me if this type" property. Potential TODO
 */
static gboolean
enzui_curl_http_src_negotiate_caps (GstEnzCurlHttpSrc * src)
{
  const GValue *response_headers;
  const GstStructure *response_struct;

  GST_INFO_OBJECT (src, "Negotiating caps...");
  if (src->caps && src->http_headers) {
    response_headers =
        gst_structure_get_value (src->http_headers, RESPONSE_HEADERS_NAME);
    if (!response_headers) {
      GST_WARNING_OBJECT (src, "Failed to get %s", RESPONSE_HEADERS_NAME);
      return FALSE;
    }
    response_struct = gst_value_get_structure (response_headers);
    if (gst_structure_has_field_typed (response_struct, "content-type",
            G_TYPE_STRING)) {
      const gchar *content_type =
          gst_structure_get_string (response_struct, "content-type");
      GST_INFO_OBJECT (src, "Setting caps as Content-Type of %s", content_type);
      src->caps = gst_caps_make_writable (src->caps);
      gst_caps_set_simple (src->caps, "content-type", G_TYPE_STRING,
          content_type, NULL);
      if (gst_base_src_set_caps (GST_BASE_SRC (src), src->caps) != TRUE) {
        GST_ERROR_OBJECT (src, "Setting caps failed!");
        return FALSE;
      }
    }
  } else {
    GST_DEBUG_OBJECT (src, "No caps have been set, continue.");
  }

  return TRUE;
}

/*
 * Cleanup the CURL easy handle once we're done with it.
 */
static inline void
enzui_curl_http_src_destroy_easy_handle (GstEnzCurlHttpSrc * src)
{
  if (src->curl_handle != NULL) {
    curl_easy_cleanup (src->curl_handle);
    src->curl_handle = NULL;
  }
  /* In addition, clean up the curl header slist if it was used. */
  if (src->slist != NULL) {
    curl_slist_free_all (src->slist);
    src->slist = NULL;
  }
}

static GstStateChangeReturn
enzui_curl_http_src_change_state (GstElement * element, GstStateChange transition)
{
  GstStateChangeReturn ret;
  GstEnzCurlHttpSrc *source = GST_CURLHTTPSRC (element);
  GSTCURL_FUNCTION_ENTRY (source);

  switch (transition) {
    case GST_STATE_CHANGE_NULL_TO_READY:
      if (!enzui_curl_http_src_ref_multi (source)) {
        GST_ELEMENT_ERROR (element, RESOURCE, OPEN_READ,
            ("The bounded media worker could not start"), (NULL));
        return GST_STATE_CHANGE_FAILURE;
      }
      break;
    case GST_STATE_CHANGE_READY_TO_PAUSED:
      if (source->uri == NULL) {
        GST_ELEMENT_ERROR (element, RESOURCE, OPEN_READ, (_("No URL set.")),
            ("Missing URL"));
        return GST_STATE_CHANGE_FAILURE;
      }
      break;
    case GST_STATE_CHANGE_READY_TO_NULL:
      GST_DEBUG_OBJECT (source, "Removing from multi_loop queue...");
      /* The pipeline has ended, so signal any running request to end
         and wait until the multi_loop has stopped using this element */
      enzui_curl_http_src_wait_until_removed (source);
      enzui_curl_http_src_unref_multi (source);
      break;
    default:
      break;
  }

  ret = GST_ELEMENT_CLASS (parent_class)->change_state (element, transition);

  GSTCURL_FUNCTION_EXIT (source);
  return ret;
}

/*
 * Take care of any memory that may be left over from the instance that's now
 * closing before we leak it.
 */
static void
enzui_curl_http_src_cleanup_instance (GstEnzCurlHttpSrc * src)
{
  gint i;
  g_mutex_lock (&src->uri_mutex);
  g_free (src->uri);
  src->uri = NULL;
  g_free (src->redirect_uri);
  src->redirect_uri = NULL;
  g_mutex_unlock (&src->uri_mutex);
  g_mutex_clear (&src->uri_mutex);

  g_free (src->username);
  src->username = NULL;
  g_free (src->password);
  src->password = NULL;

  g_free (src->proxy_uri);
  src->proxy_uri = NULL;
  g_free (src->no_proxy_list);
  src->no_proxy_list = NULL;
  g_free (src->proxy_user);
  src->proxy_user = NULL;
  g_free (src->proxy_pass);
  src->proxy_pass = NULL;

  for (i = 0; i < src->number_cookies; i++) {
    g_free (src->cookies[i]);
    src->cookies[i] = NULL;
  }
  g_free (src->cookies);
  src->cookies = NULL;

  g_free (src->user_agent);
  src->user_agent = NULL;

  g_free (src->custom_ca_file);
  src->custom_ca_file = NULL;

  g_mutex_clear (&src->multi_task_context.mutex);
  g_cond_clear (&src->multi_task_context.signal);
  g_mutex_clear (&src->buffer_mutex);

  g_cond_clear (&src->buffer_cond);

  g_free (src->buffer);
  src->buffer = NULL;

  if (src->request_headers) {
    gst_structure_free (src->request_headers);
    src->request_headers = NULL;
  }
  if (src->http_headers != NULL) {
    gst_structure_free (src->http_headers);
    src->http_headers = NULL;
  }
  g_free (src->reason_phrase);
  src->reason_phrase = NULL;
  gst_caps_replace (&src->caps, NULL);

  enzui_curl_http_src_destroy_easy_handle (src);
}

static gboolean
enzui_curl_http_src_query (GstBaseSrc * bsrc, GstQuery * query)
{
  GstEnzCurlHttpSrc *src = GST_CURLHTTPSRC (bsrc);
  gboolean ret;
  GSTCURL_FUNCTION_ENTRY (src);

  switch (GST_QUERY_TYPE (query)) {
    case GST_QUERY_SCHEDULING:
      ret = GST_BASE_SRC_CLASS (parent_class)->query (bsrc, query);
      if (ret) {
        GstSchedulingFlags flags;
        gint minimum, maximum, alignment;
        gst_query_parse_scheduling (query, &flags, &minimum, &maximum, &alignment);
        /* Private URI aliases still represent a bandwidth-limited network source. */
        gst_query_set_scheduling (query, flags | GST_SCHEDULING_FLAG_BANDWIDTH_LIMITED,
            minimum, maximum, alignment);
      }
      break;
    case GST_QUERY_URI:
      g_mutex_lock (&src->uri_mutex);
      gst_query_set_uri (query, src->uri);
      if (src->redirect_uri != NULL) {
        gst_query_set_uri_redirection (query, src->redirect_uri);
      }
      g_mutex_unlock (&src->uri_mutex);
      ret = TRUE;
      break;
    default:
      ret = GST_BASE_SRC_CLASS (parent_class)->query (bsrc, query);
      break;
  }

  GSTCURL_FUNCTION_EXIT (src);
  return ret;
}

static guint64
enzui_curl_http_src_get_length_from_headers (GstEnzCurlHttpSrc * src)
{
  const GValue *response_headers;

  if (src->http_headers == NULL)
    return 0;

  response_headers = gst_structure_get_value (src->http_headers,
      RESPONSE_HEADERS_NAME);

  /* In the case of a Range GET, the Content-Length header will contain
   * the size of range requested, and the Content-Range header should
   * have the start, stop and total size of the resource */
  if (gst_structure_has_field_typed (gst_value_get_structure (response_headers),
          "content-range", G_TYPE_STRING)) {
    const gchar *content_range =
        gst_structure_get_string (gst_value_get_structure (response_headers),
        "content-range");
    const gchar *slash = strchr (content_range, '/');
    if (slash && slash[1] != '*') {
      return g_ascii_strtoull (slash + 1, NULL, 10);
    }
  }
  if (gst_structure_has_field_typed (gst_value_get_structure (response_headers),
          "content-length", G_TYPE_STRING)) {
    const gchar *content_length =
        gst_structure_get_string (gst_value_get_structure (response_headers),
        "content-length");
    /* Adjust by request_position since the length is of just the range. Should
     * have been taken care of content-range above */
    return src->request_position + g_ascii_strtoull (content_length, NULL, 10);
  }
  return 0;
}

static gboolean
enzui_curl_http_src_get_size (GstBaseSrc * bsrc, guint64 * size)
{
  GstEnzCurlHttpSrc *src = GST_CURLHTTPSRC (bsrc);
  gboolean ret = FALSE;

  if (src->content_size == 0 && src->http_headers == NULL) {
    return FALSE;
  }
  if (src->content_size == 0) {
    src->content_size = enzui_curl_http_src_get_length_from_headers (src);
  }
  if (src->content_size > 0) {
    *size = src->content_size;
    ret = TRUE;
  }

  return ret;
}

static gboolean
enzui_curl_http_src_is_seekable (GstBaseSrc * bsrc)
{
  GstEnzCurlHttpSrc *src = GST_CURLHTTPSRC (bsrc);

  /* NOTE: if seekable is UNKNOWN, assume yes */
  return src->seekable != GSTCURL_SEEKABLE_FALSE;
}

static gboolean
enzui_curl_http_src_do_seek (GstBaseSrc * bsrc, GstSegment * segment)
{
  GstEnzCurlHttpSrc *src = GST_CURLHTTPSRC (bsrc);
  gboolean ret = TRUE;

  g_mutex_lock (&src->buffer_mutex);
  GST_DEBUG_OBJECT (src, "do_seek(%" G_GINT64_FORMAT ", %" G_GINT64_FORMAT
      ")", segment->start, segment->stop);
  if (src->state == GSTCURL_UNLOCK) {
    GST_WARNING_OBJECT (src, "Attempt to seek while unlocked");
    ret = FALSE;
    goto done;
  }

  if (src->seekable == GSTCURL_SEEKABLE_FALSE) {
    GST_WARNING_OBJECT (src, "Not seekable");
    ret = FALSE;
    goto done;
  }

  if (segment->rate < 0.0 || segment->format != GST_FORMAT_BYTES) {
    GST_WARNING_OBJECT (src, "Invalid seek segment");
    ret = FALSE;
    goto done;
  }

  if (src->content_size > 0 && segment->start >= src->content_size) {
    GST_WARNING_OBJECT (src,
        "Potentially seeking beyond end of file, might EOS immediately");
  }

  src->request_position = segment->start;
  src->stop_position = segment->stop;

  if (src->transfer_begun) {
    GST_DEBUG_OBJECT (src, "stopping existing transfer");
    g_mutex_unlock (&src->buffer_mutex);
    enzui_curl_http_src_wait_until_removed (src);
    g_mutex_lock (&src->buffer_mutex);
    src->transfer_begun = FALSE;
    enzui_clear_body (src);
  }

done:
  g_mutex_unlock (&src->buffer_mutex);
  return ret;
}

static void
enzui_curl_http_src_uri_handler_init (gpointer g_iface, gpointer iface_data G_GNUC_UNUSED)
{
  GstURIHandlerInterface *uri_iface = (GstURIHandlerInterface *) g_iface;

  uri_iface->get_type = enzui_curl_http_src_urihandler_get_type;
  uri_iface->get_protocols = enzui_curl_http_src_urihandler_get_protocols;
  uri_iface->get_uri = enzui_curl_http_src_urihandler_get_uri;
  uri_iface->set_uri = enzui_curl_http_src_urihandler_set_uri;
}

static guint
enzui_curl_http_src_urihandler_get_type (GType type G_GNUC_UNUSED)
{
  return GST_URI_SRC;
}

static const gchar *const *
enzui_curl_http_src_urihandler_get_protocols (GType type G_GNUC_UNUSED)
{
  static const gchar *protocols[] = { "enzuihttps", "enzuihttp", NULL };

  return protocols;
}

static gchar *
enzui_curl_http_src_urihandler_get_uri (GstURIHandler * handler)
{
  gchar *ret;
  GstEnzCurlHttpSrc *source;

  g_return_val_if_fail (GST_IS_URI_HANDLER (handler), NULL);
  source = GST_CURLHTTPSRC (handler);

  GSTCURL_FUNCTION_ENTRY (source);

  g_mutex_lock (&source->uri_mutex);
  ret = g_strdup (source->uri);
  g_mutex_unlock (&source->uri_mutex);

  GSTCURL_FUNCTION_EXIT (source);
  return ret;
}

static gboolean
enzui_curl_http_src_urihandler_set_uri (GstURIHandler * handler,
    const gchar * uri, GError ** error G_GNUC_UNUSED)
{
  GstEnzCurlHttpSrc *source = GST_CURLHTTPSRC (handler);
  gboolean rc;
  GSTCURL_FUNCTION_ENTRY (source);

  g_return_val_if_fail (GST_IS_URI_HANDLER (handler), FALSE);
  g_return_val_if_fail (uri != NULL, FALSE);

  gchar *transport_uri = NULL;
  if (g_str_has_prefix (uri, "enzuihttps://"))
    transport_uri = g_strconcat ("https://", uri + strlen ("enzuihttps://"), NULL);
  else if (g_str_has_prefix (uri, "enzuihttp://"))
    transport_uri = g_strconcat ("http://", uri + strlen ("enzuihttp://"), NULL);
  else
    return FALSE;
  rc = enzui_curl_http_src_update_uri (source, transport_uri);
  g_free (transport_uri);

  GSTCURL_FUNCTION_EXIT (source);
  return rc;
}

/*
 * Cancel any currently running transfer, and then signal all the loops to drop
 * any received buffers. The ::create() method should return GST_FLOW_FLUSHING.
 */
static gboolean
enzui_curl_http_src_unlock (GstBaseSrc * bsrc)
{
  GstEnzCurlHttpSrc *src = GST_CURLHTTPSRC (bsrc);
  gboolean want_removal = FALSE;

  g_mutex_lock (&src->buffer_mutex);
  if (src->state != GSTCURL_UNLOCK) {
    if (src->state == GSTCURL_OK) {
      /* A transfer is running, cancel it */
      if (src->connection_status == GSTCURL_CONNECTED) {
        src->connection_status = GSTCURL_WANT_REMOVAL;
      }
      want_removal = TRUE;
    }
    src->pending_state = src->state;
    src->state = GSTCURL_UNLOCK;
  }
  g_cond_signal (&src->buffer_cond);
  g_mutex_unlock (&src->buffer_mutex);

  if (want_removal) {
    g_mutex_lock (&src->multi_task_context.mutex);
    g_cond_signal (&src->multi_task_context.signal);
    if (src->multi_task_context.multi_handle != NULL)
      curl_multi_wakeup (src->multi_task_context.multi_handle);
    g_mutex_unlock (&src->multi_task_context.mutex);
  }

  return TRUE;
}

/*
 * Finish the unlock request above and return curlhttpsrc to the normal state.
 * This will probably be GSTCURL_DONE, and the next return from ::create() will
 * be GST_FLOW_EOS as we don't want to deliver parts of a HTTP body.
 */
static gboolean
enzui_curl_http_src_unlock_stop (GstBaseSrc * bsrc)
{
  GstEnzCurlHttpSrc *src = GST_CURLHTTPSRC (bsrc);

  g_mutex_lock (&src->buffer_mutex);
  src->state = src->pending_state;
  src->pending_state = GSTCURL_NONE;
  g_cond_signal (&src->buffer_cond);
  g_mutex_unlock (&src->buffer_mutex);

  return TRUE;
}

/*****************************************************************************
 * Curl loop task functions begin
 *****************************************************************************/
static void
enzui_curl_http_src_curl_multi_loop (gpointer thread_data)
{
  GstEnzCurlHttpSrcMultiTaskContext *context;
  GstEnzCurlHttpSrcQueueElement *qelement, *qnext;
  gint i, still_running = 0;
  CURLMsg *curl_message;
  GstEnzCurlHttpSrc *elt;
  guint active = 0;

  context = (GstEnzCurlHttpSrcMultiTaskContext *) thread_data;

  g_mutex_lock (&context->mutex);

  /* Someone is holding a reference to us, but isn't using us so to avoid
   * unnecessary clock cycle wasting, sit in a conditional wait until woken.
   */
  while (context->queue == NULL
      && context->state == GSTCURL_MULTI_LOOP_STATE_RUNNING) {
    GSTCURL_DEBUG_PRINT ("Waiting for an element to be added...");
    g_cond_wait (&context->signal, &context->mutex);
    GSTCURL_DEBUG_PRINT ("Received wake up call!");
  }
  if (context->state == GSTCURL_MULTI_LOOP_STATE_STOP) {
    GSTCURL_INFO_PRINT ("Got instruction to shut down");
    goto out;
  }

  /* check for elements that need to be started or removed */
  qelement = context->queue;
  while (qelement != NULL) {
    qnext = qelement->next;
    elt = qelement->p;
    /* NOTE: when both the buffer_mutex and multi_task_context.mutex are
       needed, multi_task_context.mutex must be acquired first */
    gboolean resume = FALSE;
    g_mutex_lock (&elt->buffer_mutex);
    if (elt->connection_status == GSTCURL_WANT_REMOVAL) {
      curl_multi_remove_handle (context->multi_handle, elt->curl_handle);
      if (elt->state == GSTCURL_UNLOCK) {
        elt->pending_state = GSTCURL_REMOVED;
      } else {
        elt->state = GSTCURL_REMOVED;
      }
      elt->connection_status = GSTCURL_NOT_CONNECTED;
      enzui_curl_http_src_remove_queue_item (&context->queue, qelement->p);
      g_cond_signal (&elt->buffer_cond);
    } else if (elt->connection_status == GSTCURL_CONNECTED) {
      active++;
      if (elt->receive_paused && elt->resume_requested && elt->buffer_len <= enzui_receive_capacity (elt) / 2) {
        elt->receive_paused = FALSE;
        elt->resume_requested = FALSE;
        resume = TRUE;
      }
      if (g_atomic_int_compare_and_exchange (&qelement->running, 0, 1)) {
        GSTCURL_DEBUG_PRINT ("Adding easy handle for URI %s", qelement->p->uri);
        curl_multi_add_handle (context->multi_handle, qelement->p->curl_handle);
      }
    }
    g_mutex_unlock (&elt->buffer_mutex);
    if (resume && curl_easy_pause (elt->curl_handle, CURLPAUSE_CONT) != CURLE_OK) {
      g_mutex_lock (&elt->buffer_mutex);
      elt->state = GSTCURL_TOTAL_ERROR;
      elt->connection_status = GSTCURL_WANT_REMOVAL;
      g_cond_signal (&elt->buffer_cond);
      g_mutex_unlock (&elt->buffer_mutex);
    }
    qelement = qnext;
  }

  if (active == 0) {
    GSTCURL_DEBUG_PRINT ("No active elements");
    goto out;
  }

  {
    gboolean cond = FALSE;
    g_mutex_unlock (&context->mutex);
    curl_multi_poll (context->multi_handle, NULL, 0, 250, NULL);
    curl_multi_perform (context->multi_handle, &still_running);

    g_mutex_lock (&context->mutex);

    /*
     * Check the CURL message buffer to find out if any transfers have
     * completed. If they have, call the signal_finished function which
     * will signal the g_cond_wait call in that calling instance.
     */
    i = 0;
    while (cond != TRUE) {
      curl_message = curl_multi_info_read (context->multi_handle, &i);
      if (curl_message == NULL) {
        cond = TRUE;
      } else if (curl_message->msg == CURLMSG_DONE) {
        /* A hack, but I have seen curl_message->easy_handle being
         * NULL randomly, so check for that. */
        if (curl_message->easy_handle != NULL) {
          curl_multi_remove_handle (context->multi_handle,
              curl_message->easy_handle);
          enzui_curl_http_src_remove_queue_handle (&context->queue,
              curl_message->easy_handle, curl_message->data.result);
        }
      }
    }
  }
out:
  g_mutex_unlock (&context->mutex);
}

/*
 * Receive headers from the remote server and put them into the http_headers
 * structure to be sent downstream when we've got them all and started receiving
 * the body (see ::_handle_response())
 */
static size_t
enzui_curl_http_src_get_header (void *data, size_t size, size_t nmemb,
    void *src)
{
  GstEnzCurlHttpSrc *s = src;
  if (size != 0 && nmemb > G_MAXSIZE / size)
    return 0;
  /* Curl header callbacks do not include a NUL terminator. */
  g_autofree gchar *header = g_strndup (data, size * nmemb);

  if (size * nmemb == 2) {
    /* blank header, skip */
    return size * nmemb;
  }

  GST_DEBUG_OBJECT (s, "Received header: %.*s", (int) nmemb - 2,
      (char *) header);

  g_mutex_lock (&s->buffer_mutex);

  if (s->state == GSTCURL_UNLOCK) {
    g_mutex_unlock (&s->buffer_mutex);
    return size * nmemb;
  }

  if (s->http_headers == NULL) {
    /* Can't do anything here, so just silently swallow the header */
    GST_DEBUG_OBJECT (s, "HTTP Headers Structure has already been sent,"
        " ignoring header");
    g_mutex_unlock (&s->buffer_mutex);
    return size * nmemb;
  }

  if (g_str_has_prefix (header, "HTTP/")) {
    /* We have a status line! */
    gchar **status_line_fields;

    /* Have we already seen a status line? If so, delete any response headers */
    if (s->status_code > 0) {
      GstStructure *empty_headers =
          gst_structure_new_empty (RESPONSE_HEADERS_NAME);
      gst_structure_remove_field (s->http_headers, RESPONSE_HEADERS_NAME);
      gst_structure_set (s->http_headers, RESPONSE_HEADERS_NAME,
          GST_TYPE_STRUCTURE, empty_headers, NULL);
      gst_structure_free (empty_headers);

    }

    /* Process the status line */
    status_line_fields = g_strsplit ((gchar *) header, " ", 3);
    if (status_line_fields == NULL) {
      GST_ERROR_OBJECT (s, "Status line processing failed!");
    } else {
      s->status_code =
          (guint) g_ascii_strtoll (status_line_fields[1], NULL, 10);
      g_free (s->reason_phrase);
      s->reason_phrase = g_strdup (status_line_fields[2]);
      g_strchomp (s->reason_phrase);
      GST_INFO_OBJECT (s, "Received status %u for request for URI %s: %s",
          s->status_code, s->uri, s->reason_phrase);
      gst_structure_set (s->http_headers, HTTP_STATUS_CODE,
          G_TYPE_UINT, s->status_code, NULL);
      g_strfreev (status_line_fields);
    }
  } else {
    /* Normal header line */
    gchar **header_tpl = g_strsplit ((gchar *) header, ": ", 2);
    if (header_tpl == NULL) {
      GST_ERROR_OBJECT (s, "Header processing failed! (%s)", (gchar *) header);
    } else {
      const GValue *gv_resp_hdrs = gst_structure_get_value (s->http_headers,
          RESPONSE_HEADERS_NAME);
      const GstStructure *response_headers =
          gst_value_get_structure (gv_resp_hdrs);
      /* Store header key lower case (g_ascii_strdown), makes searching through
       * later on easier - end applications shouldn't care, as all HTTP headers
       * are case-insensitive */
      gchar *header_key = g_ascii_strdown (header_tpl[0], -1);
      gchar *header_value;

      /* If header field already exists, append to the end */
      if (gst_structure_has_field (response_headers, header_key) == TRUE) {
        header_value = g_strdup_printf ("%s, %s",
            gst_structure_get_string (response_headers, header_key),
            header_tpl[1]);
        gst_structure_set ((GstStructure *) response_headers, header_key,
            G_TYPE_STRING, header_value, NULL);
        g_free (header_value);
      } else {
        header_value = header_tpl[1];
        gst_structure_set ((GstStructure *) response_headers, header_key,
            G_TYPE_STRING, header_value, NULL);
      }

      /* We have some special cases - deal with them here */
      if (g_strcmp0 (header_key, "content-type") == 0) {
        enzui_curl_http_src_negotiate_caps (src);
      } else if (g_strcmp0 (header_key, "accept-ranges") == 0 &&
          g_ascii_strcasecmp (header_value, "none") == 0) {
        s->seekable = GSTCURL_SEEKABLE_FALSE;
      }

      g_free (header_key);
      g_strfreev (header_tpl);
    }
  }

  s->hdrs_updated = TRUE;

  g_mutex_unlock (&s->buffer_mutex);

  return size * nmemb;
}

/*
 * Receive chunks of the requested body and pass these back to the ::create()
 * loop
 */
static size_t
enzui_curl_http_src_get_chunks (void *chunk, size_t size, size_t nmemb, void *src)
{
  GstEnzCurlHttpSrc *s = src;
  if (size != 0 && nmemb > ENZUI_RECEIVE_LIMIT / size)
    return 0;
  size_t chunk_len = size * nmemb;
  GST_TRACE_OBJECT (s,
      "Received curl chunk for URI %s of size %d", s->uri, (int) chunk_len);
  g_mutex_lock (&s->buffer_mutex);
  if (s->state == GSTCURL_UNLOCK) {
    g_mutex_unlock (&s->buffer_mutex);
    return chunk_len;
  }
  if (chunk_len > enzui_receive_capacity (s) - s->buffer_len) {
    s->receive_paused = TRUE;
    s->backpressure_count++;
    g_cond_signal (&s->buffer_cond);
    g_mutex_unlock (&s->buffer_mutex);
    return CURL_WRITEFUNC_PAUSE;
  }
  if (enzui_read_ahead_enabled (s)) {
    if (!enzui_read_ahead_io (s, s->read_ahead_write, (guint) chunk_len, chunk, TRUE)) {
      g_mutex_unlock (&s->buffer_mutex);
      GST_ELEMENT_ERROR (s, RESOURCE, WRITE,
          ("The anonymous read-ahead buffer could not be written"), (NULL));
      return 0;
    }
    s->read_ahead_write = (guint) (((guint64) s->read_ahead_write + chunk_len) % s->read_ahead_capacity);
    s->buffer_len += (guint) chunk_len;
    s->read_ahead_peak = MAX (s->read_ahead_peak, s->buffer_len);
  } else {
    s->buffer = g_realloc (s->buffer, (s->buffer_len + chunk_len + 1) * sizeof (char));
    if (s->buffer == NULL) {
      GST_ERROR_OBJECT (s, "Realloc for cURL response message failed!");
      g_mutex_unlock (&s->buffer_mutex);
      return 0;
    }
    memcpy (s->buffer + s->buffer_len, chunk, chunk_len);
    s->buffer_len += (guint) chunk_len;
    s->peak_buffered_bytes = MAX (s->peak_buffered_bytes, s->buffer_len);
  }
  g_cond_signal (&s->buffer_cond);
  g_mutex_unlock (&s->buffer_mutex);
  return chunk_len;
}

/*
 * Request a cancellation of a currently running curl handle.
 */
static void
enzui_curl_http_src_request_remove (GstEnzCurlHttpSrc * src)
{

  g_mutex_lock (&src->multi_task_context.mutex);
  g_mutex_lock (&src->buffer_mutex);
  if (src->connection_status == GSTCURL_CONNECTED) {
    src->connection_status = GSTCURL_WANT_REMOVAL;
  }
  g_mutex_unlock (&src->buffer_mutex);
  g_cond_signal (&src->multi_task_context.signal);
  if (src->multi_task_context.multi_handle != NULL)
    curl_multi_wakeup (src->multi_task_context.multi_handle);
  g_mutex_unlock (&src->multi_task_context.mutex);
}

/*
 * Request a cancellation of a currently running curl handle and
 * block this thread until the src element has been removed
 * from the queue
 */
static void
enzui_curl_http_src_wait_until_removed (GstEnzCurlHttpSrc * src)
{
  enzui_curl_http_src_request_remove (src);
  g_mutex_lock (&src->buffer_mutex);
  while (src->connection_status != GSTCURL_NOT_CONNECTED) {
    g_cond_wait (&src->buffer_cond, &src->buffer_mutex);
  }
  g_mutex_unlock (&src->buffer_mutex);
}



static gboolean
enzui_curl_plugin_init (GstPlugin *plugin)
{
  return gst_element_register (plugin, "enzuicurlhttpsrc", GST_RANK_PRIMARY, GST_TYPE_CURLHTTPSRC);
}

gboolean
enzui_register_bounded_curl_source (void)
{
  return gst_plugin_register_static (GST_VERSION_MAJOR, GST_VERSION_MINOR,
      "enzui_bounded_curl", "enzUI bounded HTTPS media source",
      enzui_curl_plugin_init, "1.0", "MIT/X11", "enzUI", "enzUI",
      "https://github.com/GStreamer/gstreamer");
}
