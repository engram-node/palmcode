package dev.palmcode.app

import android.content.Context
import android.net.Uri
import android.provider.DocumentsContract
import java.io.File

/** SAF tree copies in both directions, DocumentsContract only (no deps). */
object WorkspaceIO {

    /** Copy an OpenDocumentTree URI into destDir (recursive). Returns file count. */
    fun importTree(context: Context, treeUri: Uri, destDir: File): Int {
        destDir.mkdirs()
        val resolver = context.contentResolver
        var count = 0
        fun docUri(docId: String) = DocumentsContract.buildDocumentUriUsingTree(treeUri, docId)
        fun walk(docId: String, dir: File) {
            val children = DocumentsContract.buildChildDocumentsUriUsingTree(treeUri, docId)
            resolver.query(
                children,
                arrayOf(
                    DocumentsContract.Document.COLUMN_DOCUMENT_ID,
                    DocumentsContract.Document.COLUMN_DISPLAY_NAME,
                    DocumentsContract.Document.COLUMN_MIME_TYPE,
                ),
                null,
                null,
                null,
            )?.use { c ->
                while (c.moveToNext()) {
                    val id = c.getString(0)
                    val name = c.getString(1)
                    val mime = c.getString(2)
                    if (mime == DocumentsContract.Document.MIME_TYPE_DIR) {
                        File(dir, name).mkdirs()
                        walk(id, File(dir, name))
                    } else {
                        resolver.openInputStream(docUri(id))?.use { inp ->
                            File(dir, name).outputStream().use { out -> inp.copyTo(out) }
                        }
                        count++
                    }
                }
            }
        }
        walk(DocumentsContract.getTreeDocumentId(treeUri), destDir)
        return count
    }

    /** Display name of the tree's root document. */
    fun treeName(context: Context, treeUri: Uri): String? =
        context.contentResolver.query(
            DocumentsContract.buildDocumentUriUsingTree(
                treeUri,
                DocumentsContract.getTreeDocumentId(treeUri),
            ),
            arrayOf(DocumentsContract.Document.COLUMN_DISPLAY_NAME),
            null,
            null,
            null,
        )?.use { c -> if (c.moveToFirst()) c.getString(0) else null }

    /** Copy srcDir into the picked tree (recursive, reuses existing nodes). */
    fun exportTree(context: Context, srcDir: File, treeUri: Uri): Int {
        val resolver = context.contentResolver
        var count = 0
        fun childDoc(parentDocId: String, name: String, wantDir: Boolean): String? {
            val children = DocumentsContract.buildChildDocumentsUriUsingTree(treeUri, parentDocId)
            resolver.query(
                children,
                arrayOf(
                    DocumentsContract.Document.COLUMN_DOCUMENT_ID,
                    DocumentsContract.Document.COLUMN_DISPLAY_NAME,
                    DocumentsContract.Document.COLUMN_MIME_TYPE,
                ),
                null,
                null,
                null,
            )?.use { c ->
                while (c.moveToNext()) {
                    val isDir = c.getString(2) == DocumentsContract.Document.MIME_TYPE_DIR
                    if (c.getString(1) == name && isDir == wantDir) return c.getString(0)
                }
            }
            return null
        }

        fun ensureDoc(parentDocId: String, name: String, dir: Boolean): String? =
            childDoc(parentDocId, name, dir) ?: DocumentsContract.createDocument(
                resolver,
                DocumentsContract.buildDocumentUriUsingTree(treeUri, parentDocId),
                if (dir) DocumentsContract.Document.MIME_TYPE_DIR else "application/octet-stream",
                name,
            )?.let { DocumentsContract.getDocumentId(it) }

        fun walk(dir: File, parentDocId: String) {
            dir.listFiles()?.forEach { f ->
                if (f.isDirectory) {
                    ensureDoc(parentDocId, f.name, true)?.let { walk(f, it) }
                } else {
                    val id = ensureDoc(parentDocId, f.name, false) ?: return@forEach
                    val uri = DocumentsContract.buildDocumentUriUsingTree(treeUri, id)
                    resolver.openOutputStream(uri, "wt")?.use { out ->
                        f.inputStream().use { it.copyTo(out) }
                    }
                    count++
                }
            }
        }
        walk(srcDir, DocumentsContract.getTreeDocumentId(treeUri))
        return count
    }
}
