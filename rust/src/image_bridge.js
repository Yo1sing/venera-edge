globalThis.__images = {
    cancelled: new Set(),
    active: new Set(),
    cancel(id) {if(this.active.has(id)) this.cancelled.add(id);},
    async fetch(imageKey,sourceKey,cid,eid,thumbnail,requestId) {
        this.active.add(requestId);
        try {return await this._fetch(imageKey,sourceKey,cid,eid,thumbnail,requestId);}
        finally {this.active.delete(requestId);this.cancelled.delete(requestId);}
    },
    async _fetch(imageKey,sourceKey,cid,eid,thumbnail,requestId) {
        const source=ComicSource.sources[sourceKey];
        let config=(thumbnail ? await source?.comic?.onThumbnailLoad?.(imageKey)
            : await source?.comic?.onImageLoad?.(imageKey,cid,eid))??{};
        if(this.cancelled.delete(requestId)) throw Error('Image request cancelled');
        if(thumbnail && (config.url??imageKey).startsWith('cover.') && source) {
            const info=await source.comic.loadInfo(cid);
            return this.fetch(info.cover,sourceKey,cid,eid,true,requestId);
        }
        for(let attempt=0;;attempt++) {
            try {
                if(this.cancelled.delete(requestId)) throw Error('Image request cancelled');
                let url=config.url??imageKey;
                if(url.startsWith('//')) url='https:'+url;
                const response=await sendMessage({method:'http',http_method:config.method??'GET',url,
                    headers:config.headers??{},data:config.data,bytes:true,_requestId:requestId});
                if(response.error) throw Error(response.error);
                if(this.cancelled.delete(requestId)) throw Error('Image request cancelled');
                if(response.status>=400) throw Error('HTTP '+response.status);
                let bytes=response.body;
                if(config.onResponse) bytes=await config.onResponse(bytes);
                if(Array.isArray(bytes)) bytes=new Uint8Array(bytes).buffer;
                if(!(bytes instanceof ArrayBuffer)&&!ArrayBuffer.isView(bytes)) throw Error('Invalid image response');
                if(config.modifyImage) bytes=await sendMessage({method:'modify_image',data:bytes,script:config.modifyImage});
                return bytes;
            } catch(error) {
                if(attempt>=5||!config.onLoadFailed||String(error).includes('cancelled')) throw error;
                const replacement=await config.onLoadFailed();
                if(!replacement) throw error;
                config=replacement;
            }
        }
    }
};
