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

/*
 * This header file contains definitions only for
 */

#ifndef GSTCURLHTTPSRC_H_
#define GSTCURLHTTPSRC_H_

#include <gst/gst.h>
#include <string.h>
#include <ctype.h>
#include <stdlib.h>
#include <gst/base/gstpushsrc.h>

#include "curltask.h"

G_BEGIN_DECLS
/* #defines don't like whitespacey bits */
#define GST_TYPE_CURLHTTPSRC \
  (enzui_curl_http_src_get_type())
#define GST_CURLHTTPSRC(obj) \
  (G_TYPE_CHECK_INSTANCE_CAST((obj),GST_TYPE_CURLHTTPSRC,GstEnzCurlHttpSrc))
#define GST_CURLHTTPSRC_CLASS(klass) \
  (G_TYPE_CHECK_CLASS_CAST((klass),GST_TYPE_CURLHTTPSRC,GstEnzCurlHttpSrcClass))
#define GST_IS_CURLHTTPSRC(obj) \
  (G_TYPE_CHECK_INSTANCE_TYPE((obj),GST_TYPE_CURLHTTPSRC))
#define GST_IS_CURLHTTPSRC_CLASS(klass) \
  (G_TYPE_CHECK_CLASS_TYPE((klass),GST_TYPE_CURLHTTPSRC))

#define GSTCURL_FUNCTIONTRACE 0
#if GSTCURL_FUNCTIONTRACE
#define GSTCURL_FUNCTION_ENTRY(x) GST_DEBUG_OBJECT(x, "Entering function");
#define GSTCURL_FUNCTION_EXIT(x) GST_DEBUG_OBJECT(x, "Leaving function");
#else
#define GSTCURL_FUNCTION_ENTRY(x)
#define GSTCURL_FUNCTION_EXIT(x)
#endif
typedef struct _GstEnzCurlHttpSrc GstEnzCurlHttpSrc;
typedef struct _GstEnzCurlHttpSrcClass GstEnzCurlHttpSrcClass;
typedef struct _GstEnzCurlHttpSrcMultiTaskContext GstEnzCurlHttpSrcMultiTaskContext;
typedef struct _GstEnzCurlHttpSrcQueueElement GstEnzCurlHttpSrcQueueElement;

#define HTTP_HEADERS_NAME       "http-headers"
#define HTTP_STATUS_CODE        "http-status-code"
#define URI_NAME                "uri"
#define REQUEST_HEADERS_NAME    "request-headers"
#define RESPONSE_HEADERS_NAME   "response-headers"
#define REDIRECT_URI_NAME       "redirection-uri"

typedef enum
  {
    GSTCURL_HTTP_VERSION_1_0,
    GSTCURL_HTTP_VERSION_1_1,
#ifdef CURL_VERSION_HTTP2
    GSTCURL_HTTP_VERSION_2_0,
#endif
    GSTCURL_HTTP_NOT,           /* For future use if HTTP protocol not used! */
    GSTCURL_HTTP_VERSION_MAX
  } GstEnzCurlHttpVersion;

typedef enum _GstCGstEnzCurlHttpSrcSeekable
{
    GSTCURL_SEEKABLE_UNKNOWN,
    GSTCURL_SEEKABLE_TRUE,
    GSTCURL_SEEKABLE_FALSE
} GstCGstEnzCurlHttpSrcSeekable;

struct _GstEnzCurlHttpSrcMultiTaskContext
{
  GstTask     *task;
  GRecMutex   task_rec_mutex;
  GMutex      mutex;
  guint       refcount;
  GCond       signal;

  GstEnzCurlHttpSrcQueueElement  *queue;

  enum
  {
    GSTCURL_MULTI_LOOP_STATE_RUNNING,
    GSTCURL_MULTI_LOOP_STATE_STOP
  } state;

  /* < private > */
  CURLM *multi_handle;
};

struct _GstEnzCurlHttpSrcClass
{
  GstPushSrcClass parent_class;

};

/*
 * Our instance class.
 */
struct _GstEnzCurlHttpSrc
{
  GstPushSrc element;
  GstEnzCurlHttpSrcMultiTaskContext multi_task_context;

  /* < private > */
  GMutex uri_mutex; /* Make the URIHandler get/set thread safe */
  /*
   * Things to tell libcURL about to build up the request message.
   */
  /* Type         Name                                      Curl Option */
  gchar *uri;                   /* CURLOPT_URL */
  gchar *redirect_uri;		/* CURLINFO_REDIRECT_URL */
  gchar *username;              /* CURLOPT_USERNAME */
  gchar *password;              /* CURLOPT_PASSWORD */
  gchar *proxy_uri;             /* CURLOPT_PROXY */
  gchar *no_proxy_list;         /* CURLOPT_NOPROXY */
  gchar *proxy_user;            /* CURLOPT_PROXYUSERNAME */
  gchar *proxy_pass;            /* CURLOPT_PROXYPASSWORD */

  /* Header options */
  gchar **cookies;              /* CURLOPT_COOKIELIST */
  gint number_cookies;
  gchar *user_agent;            /* CURLOPT_USERAGENT */
  GstStructure *request_headers;  /* CURLOPT_HTTPHEADER */
  struct curl_slist *slist;
  gboolean accept_compressed_encodings; /* CURLOPT_ACCEPT_ENCODING */
  gint64 request_position;     /* Seek to this position. */
  gint64 stop_position;        /* Stop at this position. */

  /* Connection options */
  glong allow_3xx_redirect;     /* CURLOPT_FOLLOWLOCATION */
  glong max_3xx_redirects;      /* CURLOPT_MAXREDIRS */
  gboolean keep_alive;          /* CURLOPT_TCP_KEEPALIVE */
  gint timeout_secs;            /* CURLOPT_TIMEOUT */
  gboolean strict_ssl;		/* CURLOPT_SSL_VERIFYPEER */
  gchar* custom_ca_file;	/* CURLOPT_CAINFO */

  gint total_retries;
  gint retries_remaining;

  /*TODO As the following are all multi options, move these to curl task */
  guint max_connection_time;    /* */
  guint max_conns_per_server;   /* CURLMOPT_MAX_HOST_CONNECTIONS */
  guint max_conns_per_proxy;    /* ?!? */
  guint max_conns_global;       /* CURLMOPT_MAXCONNECTS */
  /* END multi options */

  /* The private source fixes the negotiated HTTP version to 1.1. */
  GstEnzCurlHttpVersion preferred_http_version;

  enum
  {
    GSTCURL_NONE,
    GSTCURL_OK,
    GSTCURL_DONE,
    GSTCURL_UNLOCK,
    GSTCURL_REMOVED,
    GSTCURL_BAD_QUEUE_REQUEST,
    GSTCURL_TOTAL_ERROR,
    GSTCURL_PIPELINE_NULL,
    GSTCURL_MAX
  } state, pending_state;
  CURL *curl_handle;
  GMutex buffer_mutex;
  GCond buffer_cond;
  gchar *buffer;
  guint buffer_len;
  gint read_ahead_fd;
  guint read_ahead_capacity;
  guint read_ahead_read;
  guint read_ahead_write;
  guint64 read_ahead_peak;
  gboolean receive_paused;
  gboolean resume_requested;
  guint64 peak_buffered_bytes;
  guint64 backpressure_count;
  gboolean transfer_begun;
  gboolean data_received;
  enum {
    GSTCURL_NOT_CONNECTED,
    GSTCURL_CONNECTED,
    GSTCURL_WANT_REMOVAL
  } connection_status;

  /*
   * Response Headers
   */
  GstStructure *http_headers;
  guint status_code;
  gchar *reason_phrase;
  gboolean hdrs_updated;
  guint64 content_size;
  GstCGstEnzCurlHttpSrcSeekable seekable;

  CURLcode curl_result;
  char curl_errbuf[CURL_ERROR_SIZE];

  GstCaps *caps;
};

GType enzui_curl_http_src_get_type (void);

G_END_DECLS
#endif /* GSTCURLHTTPSRC_H_ */
