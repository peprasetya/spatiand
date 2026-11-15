package id.prasetya.spatiand;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.OpenableColumns;
import android.webkit.MimeTypeMap;

import java.io.File;
import java.io.FileNotFoundException;
import java.io.IOException;

/**
 * Files handed to the phone's own apps by SpatiShelf's "Open with" (docs/spatiand-apps.md): a
 * file of shared storage, or one SpatiShelf fetched from a remote into Spatiand's own folders, as
 * a {@code content://id.prasetya.spatiand.files/<path>} address. Not exported: an app reads one
 * only when Spatiand has granted it that address ({@link Shell#askSystem}), and only to read.
 */
public final class OpenFiles extends ContentProvider {
    static final String AUTHORITY = "id.prasetya.spatiand.files";

    static Uri uriFor(String path) {
        return new Uri.Builder().scheme("content").authority(AUTHORITY).path(path).build();
    }

    /** The file an address names, if it is one that may be handed out. */
    private File fileOf(Uri uri) throws FileNotFoundException {
        String path = uri.getPath();
        if (path == null) throw new FileNotFoundException("no path");
        try {
            File file = new File(path).getCanonicalFile();
            String at = file.getPath();
            String[] roots = {
                "/storage/",
                getContext().getFilesDir().getCanonicalPath() + "/",
                getContext().getCacheDir().getCanonicalPath() + "/",
            };
            for (String root : roots) {
                if (at.startsWith(root)) return file;
            }
        } catch (IOException e) {
            throw new FileNotFoundException(e.toString());
        }
        throw new FileNotFoundException("not a file that is shared: " + path);
    }

    @Override
    public boolean onCreate() {
        return true;
    }

    @Override
    public ParcelFileDescriptor openFile(Uri uri, String mode) throws FileNotFoundException {
        if (!"r".equals(mode)) throw new SecurityException("only to be read");
        return ParcelFileDescriptor.open(fileOf(uri), ParcelFileDescriptor.MODE_READ_ONLY);
    }

    @Override
    public Cursor query(Uri uri, String[] projection, String selection, String[] selectionArgs, String sortOrder) {
        File file;
        try {
            file = fileOf(uri);
        } catch (FileNotFoundException e) {
            return null;
        }
        String[] columns = projection != null ? projection : new String[] {OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE};
        MatrixCursor cursor = new MatrixCursor(columns, 1);
        Object[] row = new Object[columns.length];
        for (int i = 0; i < columns.length; i++) {
            if (OpenableColumns.DISPLAY_NAME.equals(columns[i])) row[i] = file.getName();
            else if (OpenableColumns.SIZE.equals(columns[i])) row[i] = file.length();
        }
        cursor.addRow(row);
        return cursor;
    }

    @Override
    public String getType(Uri uri) {
        return typeOf(uri.getPath());
    }

    static String typeOf(String path) {
        String name = path == null ? "" : path;
        int dot = name.lastIndexOf('.');
        String type = dot < 0 ? null : MimeTypeMap.getSingleton().getMimeTypeFromExtension(name.substring(dot + 1).toLowerCase());
        return type != null ? type : "application/octet-stream";
    }

    @Override
    public Uri insert(Uri uri, ContentValues values) {
        throw new UnsupportedOperationException("read only");
    }

    @Override
    public int update(Uri uri, ContentValues values, String selection, String[] selectionArgs) {
        throw new UnsupportedOperationException("read only");
    }

    @Override
    public int delete(Uri uri, String selection, String[] selectionArgs) {
        throw new UnsupportedOperationException("read only");
    }
}
