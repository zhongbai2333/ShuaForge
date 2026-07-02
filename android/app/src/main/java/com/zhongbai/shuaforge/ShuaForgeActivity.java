package com.zhongbai.shuaforge;

import android.app.NativeActivity;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.Bundle;
import android.provider.OpenableColumns;
import android.util.Log;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;

public class ShuaForgeActivity extends NativeActivity {
    private static final String TAG = "ShuaForgeActivity";
    private static final int REQUEST_IMPORT_PROBLEM_BANK = 4301;

    static {
        System.loadLibrary("shuaforge_core");
    }

    private static native void nativeOnProblemBankFilePickerResult(String path, String error);

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
    }

    public void openProblemBankFilePicker() {
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.setType("*/*");
        intent.putExtra(Intent.EXTRA_MIME_TYPES, new String[] {
                "application/json",
                "text/json",
                "text/csv",
                "text/comma-separated-values",
                "application/zip",
                "application/x-zip-compressed",
                "text/*"
        });
        intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);

        try {
            startActivityForResult(intent, REQUEST_IMPORT_PROBLEM_BANK);
        } catch (Exception error) {
            Log.e(TAG, "Failed to open problem bank file picker", error);
            nativeOnProblemBankFilePickerResult(null, "打开系统文件选择器失败：" + error.getMessage());
        }
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode != REQUEST_IMPORT_PROBLEM_BANK) {
            return;
        }

        if (resultCode != RESULT_OK || data == null || data.getData() == null) {
            nativeOnProblemBankFilePickerResult(null, "已取消选择题库文件。");
            return;
        }

        Uri uri = data.getData();
        try {
            File cachedFile = copyUriToCache(uri);
            nativeOnProblemBankFilePickerResult(cachedFile.getAbsolutePath(), null);
        } catch (Exception error) {
            Log.e(TAG, "Failed to import selected problem bank", error);
            nativeOnProblemBankFilePickerResult(null, "读取所选文件失败：" + error.getMessage());
        }
    }

    private File copyUriToCache(Uri uri) throws Exception {
        File importDir = new File(getCacheDir(), "problem-bank-imports");
        if (!importDir.exists() && !importDir.mkdirs()) {
            throw new IllegalStateException("无法创建导入缓存目录");
        }

        String displayName = queryDisplayName(uri);
        String safeName = sanitizeFileName(displayName);
        if (safeName.isEmpty()) {
            safeName = "problem-bank";
        }
        File output = new File(importDir, System.currentTimeMillis() + "-" + safeName);

        try (InputStream input = getContentResolver().openInputStream(uri);
             FileOutputStream outputStream = new FileOutputStream(output)) {
            if (input == null) {
                throw new IllegalStateException("无法打开所选文件");
            }
            byte[] buffer = new byte[64 * 1024];
            int read;
            while ((read = input.read(buffer)) != -1) {
                outputStream.write(buffer, 0, read);
            }
        }

        return output;
    }

    private String queryDisplayName(Uri uri) {
        try (Cursor cursor = getContentResolver().query(uri, null, null, null, null)) {
            if (cursor != null && cursor.moveToFirst()) {
                int index = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME);
                if (index >= 0) {
                    String name = cursor.getString(index);
                    if (name != null) {
                        return name;
                    }
                }
            }
        } catch (Exception error) {
            Log.w(TAG, "Failed to query selected file display name", error);
        }
        String fallback = uri.getLastPathSegment();
        return fallback == null ? "problem-bank" : fallback;
    }

    private String sanitizeFileName(String name) {
        return name.replaceAll("[\\\\/:*?\"<>|\\p{Cntrl}]", "_").trim();
    }
}
