package app.nitch.tv

import android.app.Activity
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import android.os.Bundle
import android.view.KeyEvent
import android.view.WindowManager
import android.webkit.JavascriptInterface
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient
import android.window.OnBackInvokedDispatcher
import org.json.JSONArray
import org.json.JSONObject

/**
 * Nitch for Android TV: a thin shell around a WebView (D4). The UI is the web app served by the
 * Nitch server, so it updates with the server. The shell finds the server (mDNS `_nitch._tcp` or a
 * typed address), remembers it, forwards the remote's Back and media keys to the page, and offers
 * a tiny bridge (`window.NitchTV`): exit, keep the screen on, change the server.
 */
class MainActivity : Activity() {
    private lateinit var web: WebView
    private val prefs by lazy { getSharedPreferences("nitch", MODE_PRIVATE) }
    private val nsd by lazy { getSystemService(NSD_SERVICE) as NsdManager }
    private var discovery: NsdManager.DiscoveryListener? = null
    private val found = linkedMapOf<String, String>() // name -> http://host:port

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        web = WebView(this).apply {
            settings.javaScriptEnabled = true
            settings.domStorageEnabled = true
            settings.mediaPlaybackRequiresUserGesture = false
            // The web app switches to TV mode when it sees this.
            settings.userAgentString = settings.userAgentString + " NitchTV/" + BuildConfigVersion
            addJavascriptInterface(Bridge(), "NitchTV")
            webViewClient = object : WebViewClient() {
                override fun onReceivedError(view: WebView, request: WebResourceRequest, error: WebResourceError) {
                    // The server isn't reachable: back to the connect page.
                    if (request.isForMainFrame && !request.url.toString().startsWith("file:")) {
                        showConnect("Can't reach ${request.url.host}:${request.url.port} (${error.description})")
                    }
                }
            }
            isFocusable = true
            isFocusableInTouchMode = true
        }
        setContentView(web)
        // Android 13+ delivers Back through this callback (from Android 16 on, only through it),
        // never as a key event; the page decides what Back means.
        if (Build.VERSION.SDK_INT >= 33) {
            onBackInvokedDispatcher.registerOnBackInvokedCallback(OnBackInvokedDispatcher.PRIORITY_DEFAULT) {
                sendToPage("nitch:back")
            }
        }
        // The page comes from the server and changes with it: a cached old index.html would point
        // at script files that no longer exist (a blank screen), so start without any cache.
        web.clearCache(true)
        val server = prefs.getString(KEY_SERVER, null)
        if (server != null) web.loadUrl(server) else showConnect(null)
    }

    private fun showConnect(error: String?) {
        val query = if (error != null) "?error=" + android.net.Uri.encode(error) else ""
        web.loadUrl("file:///android_asset/connect.html$query")
        startDiscovery()
        // Couldn't reach the saved server (network not up yet, server restarting): keep trying it
        // while the connect page is shown, so the app recovers on its own.
        if (error != null) web.postDelayed(::retrySaved, RETRY_MS)
    }

    private fun retrySaved() {
        val server = prefs.getString(KEY_SERVER, null) ?: return
        if (web.url?.startsWith("file:///android_asset/connect.html") == true) web.loadUrl(server)
    }

    // ---------- remote keys ----------

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        val name = when (event.keyCode) {
            // Android 13+: Back goes through the callback registered in onCreate.
            KeyEvent.KEYCODE_BACK -> if (Build.VERSION.SDK_INT >= 33) return super.dispatchKeyEvent(event) else "nitch:back"
            KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE, KeyEvent.KEYCODE_MEDIA_PLAY, KeyEvent.KEYCODE_MEDIA_PAUSE -> "nitch:playpause"
            else -> return super.dispatchKeyEvent(event)
        }
        // The page decides what Back means (close player, go back, exit via NitchTV.exit()).
        if (event.action == KeyEvent.ACTION_UP) sendToPage(name)
        return true
    }

    private fun sendToPage(event: String) {
        web.evaluateJavascript("window.dispatchEvent(new Event('$event'))", null)
    }

    // ---------- server discovery ----------

    private fun startDiscovery() {
        if (discovery != null) return
        val listener = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(serviceType: String) {}
            override fun onDiscoveryStopped(serviceType: String) {}
            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
                discovery = null
            }
            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {}
            override fun onServiceLost(info: NsdServiceInfo) {}
            override fun onServiceFound(info: NsdServiceInfo) {
                @Suppress("DEPRECATION")
                nsd.resolveService(info, object : NsdManager.ResolveListener {
                    override fun onResolveFailed(info: NsdServiceInfo, errorCode: Int) {}
                    override fun onServiceResolved(info: NsdServiceInfo) {
                        @Suppress("DEPRECATION")
                        val host = info.host?.hostAddress ?: return
                        val url = "http://" + (if (host.contains(':')) "[$host]" else host) + ":" + info.port + "/"
                        runOnUiThread {
                            found[info.serviceName + " (" + host + ")"] = url
                            pushServers()
                        }
                    }
                })
            }
        }
        discovery = listener
        nsd.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, listener)
    }

    private fun stopDiscovery() {
        discovery?.let { runCatching { nsd.stopServiceDiscovery(it) } }
        discovery = null
    }

    private fun pushServers() {
        val list = JSONArray()
        found.forEach { (name, url) -> list.put(JSONObject().put("name", name).put("url", url)) }
        web.evaluateJavascript("window.nitchServers && window.nitchServers($list)", null)
    }

    override fun onDestroy() {
        stopDiscovery()
        super.onDestroy()
    }

    // ---------- bridge for the page ----------

    inner class Bridge {
        @JavascriptInterface
        fun exit() = runOnUiThread { finish() }

        @JavascriptInterface
        fun setKeepScreenOn(on: Boolean) = runOnUiThread {
            if (on) window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            else window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        }

        /** From the connect page: remember `address` and open it. */
        @JavascriptInterface
        fun connect(address: String) = runOnUiThread {
            var url = address.trim()
            if (!url.startsWith("http://") && !url.startsWith("https://")) url = "http://$url"
            // No port given: the server's default.
            if (url.startsWith("http://") && android.net.Uri.parse(url).port == -1) url = url.trimEnd('/') + ":8080"
            if (!url.endsWith("/")) url += "/"
            prefs.edit().putString(KEY_SERVER, url).apply()
            stopDiscovery()
            web.loadUrl(url)
        }

        /** From the web app's settings: forget the server and show the connect page. */
        @JavascriptInterface
        fun changeServer() = runOnUiThread {
            prefs.edit().remove(KEY_SERVER).apply()
            showConnect(null)
        }

        @JavascriptInterface
        fun servers() = runOnUiThread { pushServers() }
    }

    companion object {
        const val SERVICE_TYPE = "_nitch._tcp"
        const val KEY_SERVER = "server"
        const val RETRY_MS = 5_000L
        const val BuildConfigVersion = "0.1.0"
    }
}
