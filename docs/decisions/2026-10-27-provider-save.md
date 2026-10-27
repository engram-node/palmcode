# Save provider credentials as one acknowledged profile

Encrypt a replacement key before changing preferences, then synchronously
commit ciphertext, IV and provider fields in one editor transaction on an IO
dispatcher. Keep the existing Keystore alias and preference format so installed
credentials remain readable. A failed commit reports failure and attempts to
restore the previous cached profile; repeated disk failures cannot guarantee
durable rollback. No raw exception text is displayed.

Saving credentials does not require a model. Setup remains incomplete until a
main model is entered. Test connection uses the same persistence path before
configuring the provider. A configuration failure is distinguished from a save
failure. Replacement byte buffers are cleared in finally blocks.

Keep the password field empty when reopening Settings. Show an explicit stored
key status and preserve that key when saving with the field empty. Prefilling
decrypted credentials was rejected because it unnecessarily exposes secrets to
the UI. Independent asynchronous field writes were rejected because encryption
failure could otherwise leave a saved URL paired with an old or missing key.

Regression coverage uses injected preferences/cipher failures on the JVM and
dedicated preferences in the isolated bench package for real Android Keystore
round trips across a process restart. It never overwrites the owner's key.
