package com.kh1ng.gah;

import android.annotation.SuppressLint;
import android.app.Activity;
import android.content.res.ColorStateList;
import android.content.Intent;
import android.graphics.Color;
import android.net.Uri;
import android.os.Bundle;
import android.view.Gravity;
import android.view.View;
import android.webkit.CookieManager;
import android.webkit.WebResourceError;
import android.webkit.WebResourceRequest;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.Button;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.TextView;

import java.net.URI;

public final class MainActivity extends Activity {
    private static final String PREFS = "gah-controller";
    private static final String CENTRAL_URL = "central-url";
    private EditText address;
    private TextView status;
    private LinearLayout setup;
    private WebView web;
    private Button changeServer;
    private String trustedUrl;

    @SuppressLint("SetJavaScriptEnabled")
    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        LinearLayout root = column();
        root.setOnApplyWindowInsetsListener((view, insets) -> {
            view.setPadding(0, insets.getSystemWindowInsetTop(), 0, insets.getSystemWindowInsetBottom());
            return insets;
        });
        LinearLayout bar = new LinearLayout(this);
        bar.setGravity(Gravity.CENTER_VERTICAL);
        bar.setPadding(dp(16), dp(8), dp(8), dp(8));
        TextView title = new TextView(this);
        title.setText("GAH");
        title.setTextSize(20);
        title.setTypeface(null, android.graphics.Typeface.BOLD);
        bar.addView(title, new LinearLayout.LayoutParams(0, dp(48), 1));
        changeServer = new Button(this);
        changeServer.setText(R.string.change_server);
        changeServer.setAllCaps(false);
        changeServer.setVisibility(View.GONE);
        changeServer.setOnClickListener(ignored -> showSetup(null));
        bar.addView(changeServer);
        root.addView(bar);

        setup = column();
        setup.setPadding(dp(24), dp(32), dp(24), dp(24));
        TextView heading = new TextView(this);
        heading.setText("Connect to your central node");
        heading.setTextSize(24);
        heading.setTypeface(null, android.graphics.Typeface.BOLD);
        setup.addView(heading);
        TextView explanation = new TextView(this);
        explanation.setText("This phone is a control surface only. It does not run workers or store server data.");
        explanation.setTextSize(16);
        explanation.setPadding(0, dp(8), 0, dp(20));
        setup.addView(explanation);
        address = new EditText(this);
        address.setHint(R.string.server_hint);
        address.setContentDescription(getString(R.string.server_address));
        address.setInputType(android.text.InputType.TYPE_CLASS_TEXT | android.text.InputType.TYPE_TEXT_VARIATION_URI);
        address.setSingleLine(true);
        setup.addView(address, new LinearLayout.LayoutParams(-1, dp(56)));
        Button connect = new Button(this);
        connect.setText(R.string.connect);
        connect.setAllCaps(false);
        connect.setTextColor(Color.WHITE);
        connect.setBackgroundTintList(ColorStateList.valueOf(Color.rgb(79, 70, 229)));
        connect.setOnClickListener(ignored -> connect());
        LinearLayout.LayoutParams connectLayout = new LinearLayout.LayoutParams(-1, dp(56));
        connectLayout.topMargin = dp(12);
        setup.addView(connect, connectLayout);
        status = new TextView(this);
        status.setTextSize(15);
        status.setTextColor(Color.rgb(190, 40, 40));
        status.setPadding(0, dp(12), 0, 0);
        status.setAccessibilityLiveRegion(View.ACCESSIBILITY_LIVE_REGION_POLITE);
        setup.addView(status);
        root.addView(setup, new LinearLayout.LayoutParams(-1, 0, 1));

        web = new WebView(this);
        WebSettings settings = web.getSettings();
        settings.setJavaScriptEnabled(true);
        settings.setDomStorageEnabled(true);
        settings.setAllowFileAccess(false);
        settings.setAllowContentAccess(false);
        settings.setMixedContentMode(WebSettings.MIXED_CONTENT_NEVER_ALLOW);
        settings.setUserAgentString(settings.getUserAgentString() + " GAH-Android/0.1");
        CookieManager.getInstance().setAcceptCookie(true);
        CookieManager.getInstance().setAcceptThirdPartyCookies(web, false);
        WebView.setWebContentsDebuggingEnabled((getApplicationInfo().flags & android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE) != 0);
        web.setWebViewClient(new WebViewClient() {
            @Override public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) {
                Uri target = request.getUrl();
                if (trustedUrl != null && CentralUrl.sameOrigin(trustedUrl, URI.create(target.toString()))) return false;
                if ("http".equals(target.getScheme()) || "https".equals(target.getScheme())) startActivity(new Intent(Intent.ACTION_VIEW, target));
                return true;
            }
            @Override public void onReceivedError(WebView view, WebResourceRequest request, WebResourceError error) {
                if (request.isForMainFrame()) showSetup("Could not reach the central node. Check the address and network, then try again.");
            }
        });
        web.setVisibility(View.GONE);
        root.addView(web, new LinearLayout.LayoutParams(-1, 0, 1));
        setContentView(root);
        String saved = getSharedPreferences(PREFS, MODE_PRIVATE).getString(CENTRAL_URL, "");
        address.setText(saved);
        if (!saved.isEmpty()) connect();
    }

    private void connect() {
        try {
            trustedUrl = CentralUrl.normalize(address.getText().toString());
            getSharedPreferences(PREFS, MODE_PRIVATE).edit().putString(CENTRAL_URL, trustedUrl).apply();
            status.setText("");
            setup.setVisibility(View.GONE);
            web.setVisibility(View.VISIBLE);
            changeServer.setVisibility(View.VISIBLE);
            web.loadUrl(trustedUrl);
        } catch (IllegalArgumentException error) {
            status.setText(error.getMessage());
        }
    }

    private void showSetup(String message) {
        web.setVisibility(View.GONE);
        setup.setVisibility(View.VISIBLE);
        changeServer.setVisibility(View.GONE);
        status.setText(message == null ? "" : message);
        address.requestFocus();
    }

    @Override public void onBackPressed() {
        if (web.getVisibility() == View.VISIBLE && web.canGoBack()) web.goBack();
        else if (web.getVisibility() == View.VISIBLE) showSetup(null);
        else super.onBackPressed();
    }

    @Override protected void onDestroy() { web.destroy(); super.onDestroy(); }

    private LinearLayout column() { LinearLayout layout = new LinearLayout(this); layout.setOrientation(LinearLayout.VERTICAL); return layout; }
    private int dp(int value) { return Math.round(value * getResources().getDisplayMetrics().density); }
}
